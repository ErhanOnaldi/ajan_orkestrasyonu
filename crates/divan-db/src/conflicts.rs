//! File-touch conflict detection (spec §4 `file_touches`, impl plan §F2.6).
//!
//! hcom's pattern: when two different agents touch the same path within a short
//! window (default 30s), the hub raises a conflict alert. This module records
//! touches and finds the conflicting agents; raising the alert message is the
//! `MessageBus`'s job (daemon).

use crate::store::{Db, DbResult};
use divan_core::{AgentId, TaskId};
use rusqlite::params;
use std::collections::BTreeSet;

/// Default conflict window in milliseconds (hcom's 30s, spec §4 comment).
pub const CONFLICT_WINDOW_MS: i64 = 30_000;

/// File-touch repository surface (impl plan §F2.6).
pub trait ConflictStore {
    /// Record that `agent` (for `task`) touched `path` at `ts`.
    fn record_touch(
        &self,
        path: &str,
        agent: &AgentId,
        task: Option<&TaskId>,
        ts: i64,
    ) -> DbResult<()>;

    /// Other agents (≠ `agent`) that touched `path` within `[ts - window, ts]`.
    /// Non-empty result means a conflict at `path`.
    fn conflicting_agents(
        &self,
        path: &str,
        agent: &AgentId,
        ts: i64,
        window_ms: i64,
    ) -> DbResult<Vec<AgentId>>;
}

impl ConflictStore for Db {
    fn record_touch(
        &self,
        path: &str,
        agent: &AgentId,
        task: Option<&TaskId>,
        ts: i64,
    ) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO file_touches (path, agent_id, task_id, ts) VALUES (?1,?2,?3,?4)",
            params![path, agent.as_str(), task.map(|t| t.as_str()), ts],
        )?;
        Ok(())
    }

    fn conflicting_agents(
        &self,
        path: &str,
        agent: &AgentId,
        ts: i64,
        window_ms: i64,
    ) -> DbResult<Vec<AgentId>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT agent_id FROM file_touches
             WHERE path = ?1 AND agent_id <> ?2 AND ts >= ?3 AND ts <= ?4",
        )?;
        let lo = ts - window_ms;
        let rows = stmt.query_map(params![path, agent.as_str(), lo, ts], |r| {
            r.get::<_, String>(0)
        })?;
        let set: BTreeSet<String> = rows.collect::<rusqlite::Result<_>>()?;
        Ok(set.into_iter().map(AgentId::new).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_different_agent_within_window_conflicts() {
        let db = Db::open_in_memory().unwrap();
        let a = AgentId::new("claude-1");
        let b = AgentId::new("codex-1");
        db.record_touch("src/x.rs", &a, None, 1000).unwrap();
        // b touches the same path 5s later => conflict for b.
        let c = db
            .conflicting_agents("src/x.rs", &b, 6000, CONFLICT_WINDOW_MS)
            .unwrap();
        assert_eq!(c, vec![a.clone()]);
    }

    #[test]
    fn outside_window_no_conflict() {
        let db = Db::open_in_memory().unwrap();
        let a = AgentId::new("claude-1");
        let b = AgentId::new("codex-1");
        db.record_touch("src/x.rs", &a, None, 1000).unwrap();
        // b touches 40s later — outside the 30s window.
        let c = db
            .conflicting_agents("src/x.rs", &b, 41_000, CONFLICT_WINDOW_MS)
            .unwrap();
        assert!(c.is_empty());
    }

    #[test]
    fn same_agent_does_not_conflict_with_itself() {
        let db = Db::open_in_memory().unwrap();
        let a = AgentId::new("claude-1");
        db.record_touch("src/x.rs", &a, None, 1000).unwrap();
        let c = db
            .conflicting_agents("src/x.rs", &a, 2000, CONFLICT_WINDOW_MS)
            .unwrap();
        assert!(c.is_empty());
    }

    #[test]
    fn different_paths_do_not_conflict() {
        let db = Db::open_in_memory().unwrap();
        let a = AgentId::new("claude-1");
        let b = AgentId::new("codex-1");
        db.record_touch("src/x.rs", &a, None, 1000).unwrap();
        let c = db
            .conflicting_agents("src/y.rs", &b, 2000, CONFLICT_WINDOW_MS)
            .unwrap();
        assert!(c.is_empty());
    }
}

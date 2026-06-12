//! Subscription repository + matching (K6, spec §4 `subscriptions`, impl §F2.4).

use crate::store::{Db, DbResult};
use divan_core::{AgentId, EventFilter, Subscription};
use rusqlite::{params, Row};
use std::collections::BTreeSet;

/// Subscription repository surface (impl plan §F2.4).
pub trait SubscriptionStore {
    /// Insert or replace a subscription (PK = agent_id + event_kind).
    fn subscribe(&self, sub: &Subscription) -> DbResult<()>;
    fn unsubscribe(&self, agent_id: &AgentId, event_kind: &str) -> DbResult<()>;
    fn list_subscriptions(&self) -> DbResult<Vec<Subscription>>;
    /// Distinct agents whose subscription matches a broadcast of `kind` from
    /// `from_agent` (optionally scoped to `task_id`). The sender is excluded —
    /// an agent never receives its own broadcast (K6).
    fn match_subscribers(
        &self,
        kind: &str,
        task_id: Option<&str>,
        from_agent: &AgentId,
    ) -> DbResult<Vec<AgentId>>;
}

fn row_to_sub(row: &Row<'_>) -> rusqlite::Result<Subscription> {
    let filter: Option<String> = row.get("filter")?;
    Ok(Subscription {
        agent_id: AgentId::new(row.get::<_, String>("agent_id")?),
        event_kind: row.get("event_kind")?,
        filter: filter.and_then(|f| serde_json::from_str::<EventFilter>(&f).ok()),
    })
}

impl SubscriptionStore for Db {
    fn subscribe(&self, sub: &Subscription) -> DbResult<()> {
        let conn = self.lock()?;
        let filter = match &sub.filter {
            Some(f) => Some(serde_json::to_string(f)?),
            None => None,
        };
        conn.execute(
            "INSERT INTO subscriptions (agent_id, event_kind, filter) VALUES (?1,?2,?3)
             ON CONFLICT(agent_id, event_kind) DO UPDATE SET filter = excluded.filter",
            params![sub.agent_id.as_str(), sub.event_kind, filter],
        )?;
        Ok(())
    }

    fn unsubscribe(&self, agent_id: &AgentId, event_kind: &str) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM subscriptions WHERE agent_id = ?1 AND event_kind = ?2",
            params![agent_id.as_str(), event_kind],
        )?;
        Ok(())
    }

    fn list_subscriptions(&self) -> DbResult<Vec<Subscription>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM subscriptions ORDER BY agent_id, event_kind")?;
        let rows = stmt.query_map([], row_to_sub)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn match_subscribers(
        &self,
        kind: &str,
        task_id: Option<&str>,
        from_agent: &AgentId,
    ) -> DbResult<Vec<AgentId>> {
        // Matching is deterministic, in-code (K2): load subs, apply the same
        // Subscription::matches the core defines. Dedupe + exclude the sender.
        let subs = self.list_subscriptions()?;
        let mut out = BTreeSet::new();
        for s in subs {
            if &s.agent_id != from_agent && s.matches(kind, task_id, from_agent.as_str()) {
                out.insert(s.agent_id.into_string());
            }
        }
        Ok(out.into_iter().map(AgentId::new).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(agent: &str, kind: &str, filter: Option<EventFilter>) -> Subscription {
        Subscription {
            agent_id: AgentId::new(agent),
            event_kind: kind.into(),
            filter,
        }
    }

    #[test]
    fn subscribe_upsert_and_list() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&sub("codex-1", "question", None)).unwrap();
        db.subscribe(&sub(
            "codex-1",
            "question",
            Some(EventFilter {
                task_id: Some("t1".into()),
                from_agent: None,
            }),
        ))
        .unwrap();
        let all = db.list_subscriptions().unwrap();
        assert_eq!(all.len(), 1, "same (agent,kind) upserts");
        assert_eq!(
            all[0].filter.as_ref().unwrap().task_id.as_deref(),
            Some("t1")
        );
    }

    #[test]
    fn match_excludes_sender_and_nonsubscribers() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&sub("codex-1", "question", None)).unwrap();
        db.subscribe(&sub("claude-1", "question", None)).unwrap(); // sender
        db.subscribe(&sub("agy-1", "status", None)).unwrap(); // different kind
        let subs = db
            .match_subscribers("question", None, &AgentId::new("claude-1"))
            .unwrap();
        assert_eq!(subs, vec![AgentId::new("codex-1")]);
    }

    #[test]
    fn filter_scopes_matching() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&sub(
            "codex-1",
            "*",
            Some(EventFilter {
                task_id: Some("impl-1".into()),
                from_agent: None,
            }),
        ))
        .unwrap();
        let from = AgentId::new("claude-1");
        assert_eq!(
            db.match_subscribers("question", Some("impl-1"), &from)
                .unwrap(),
            vec![AgentId::new("codex-1")]
        );
        assert!(db
            .match_subscribers("question", Some("impl-2"), &from)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unsubscribe_removes() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&sub("codex-1", "question", None)).unwrap();
        db.unsubscribe(&AgentId::new("codex-1"), "question")
            .unwrap();
        assert!(db.list_subscriptions().unwrap().is_empty());
    }
}

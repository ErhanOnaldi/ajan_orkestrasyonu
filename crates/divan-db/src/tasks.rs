//! Task repository + the transactional state machine (impl plan §5.2, §F1.2).
//!
//! `transition` is the ONLY way task state changes (spec §4.2): it validates the
//! transition, then writes the new state AND a `task_transition` trace event in
//! a single atomic transaction (impl plan §F1.2 acceptance).

use crate::store::{Db, DbError, DbResult};
use divan_core::{
    AgentId, DivanError, Task, TaskId, TaskKind, TaskState, TraceEventKind, TransitionReason,
};
use rusqlite::{params, Row};

/// Task repository surface (impl plan §F1.2).
pub trait TaskStore {
    fn create_task(&self, task: &Task) -> DbResult<()>;
    fn get_task(&self, id: &TaskId) -> DbResult<Option<Task>>;
    fn list_tasks(&self) -> DbResult<Vec<Task>>;
    /// Record a dependency edge: `task_id` is blocked by `blocked_by`.
    fn add_dep(&self, task_id: &TaskId, blocked_by: &TaskId) -> DbResult<()>;
    /// True if every dependency of `task_id` is in `done` (linear DAG, Faz 1).
    fn deps_satisfied(&self, task_id: &TaskId) -> DbResult<bool>;
    /// Assign an agent (and optionally a worktree) to a task.
    fn assign(&self, id: &TaskId, agent: &AgentId, worktree: Option<&str>) -> DbResult<()>;
    /// Validate + apply a state transition, writing a trace event atomically.
    fn transition(&self, id: &TaskId, to: TaskState, reason: TransitionReason) -> DbResult<()>;
}

fn row_to_task(row: &Row<'_>) -> rusqlite::Result<Task> {
    let kind: String = row.get("kind")?;
    let state: String = row.get("state")?;
    let kind_v: TaskKind =
        serde_json::from_value(serde_json::Value::String(kind)).unwrap_or(TaskKind::Implement);
    let state_v: TaskState =
        serde_json::from_value(serde_json::Value::String(state)).unwrap_or(TaskState::Open);
    Ok(Task {
        id: TaskId::new(row.get::<_, String>("id")?),
        parent_id: row.get::<_, Option<String>>("parent_id")?.map(TaskId::new),
        kind: kind_v,
        title: row.get("title")?,
        spec_ref: row.get("spec_ref")?,
        state: state_v,
        assignee: row.get::<_, Option<String>>("assignee")?.map(AgentId::new),
        worktree: row.get("worktree")?,
        max_runtime_secs: row
            .get::<_, Option<i64>>("max_runtime_secs")?
            .map(|v| v as u32),
        trace_id: divan_core::TraceId::new(row.get::<_, String>("trace_id")?),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl TaskStore for Db {
    fn create_task(&self, task: &Task) -> DbResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tasks
               (id, parent_id, kind, title, spec_ref, state, assignee, worktree,
                max_runtime_secs, trace_id, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                task.id.as_str(),
                task.parent_id.as_ref().map(|p| p.as_str()),
                task.kind.as_str(),
                task.title,
                task.spec_ref,
                task.state.as_str(),
                task.assignee.as_ref().map(|a| a.as_str()),
                task.worktree,
                task.max_runtime_secs.map(|v| v as i64),
                task.trace_id.as_str(),
                task.created_at,
                task.updated_at,
            ],
        )?;
        // task_created trace event in the same tx (impl plan §1.7).
        tx.execute(
            "INSERT INTO trace_events (trace_id, event, data, ts) VALUES (?1,?2,?3,?4)",
            params![
                task.trace_id.as_str(),
                TraceEventKind::TaskCreated.as_str(),
                serde_json::json!({"task_id": task.id.as_str(), "kind": task.kind.as_str()})
                    .to_string(),
                task.created_at,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get_task(&self, id: &TaskId) -> DbResult<Option<Task>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM tasks WHERE id = ?1")?;
        let mut rows = stmt.query_map([id.as_str()], row_to_task)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn list_tasks(&self) -> DbResult<Vec<Task>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM tasks ORDER BY created_at, id")?;
        let rows = stmt.query_map([], row_to_task)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn add_dep(&self, task_id: &TaskId, blocked_by: &TaskId) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO task_deps (task_id, blocked_by) VALUES (?1, ?2)",
            params![task_id.as_str(), blocked_by.as_str()],
        )?;
        Ok(())
    }

    fn deps_satisfied(&self, task_id: &TaskId) -> DbResult<bool> {
        let conn = self.lock()?;
        // Count deps not yet done. Zero => satisfied (also true with no deps).
        let unmet: i64 = conn.query_row(
            "SELECT count(*) FROM task_deps d
               JOIN tasks t ON t.id = d.blocked_by
             WHERE d.task_id = ?1 AND t.state <> 'done'",
            [task_id.as_str()],
            |r| r.get(0),
        )?;
        Ok(unmet == 0)
    }

    fn assign(&self, id: &TaskId, agent: &AgentId, worktree: Option<&str>) -> DbResult<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE tasks SET assignee = ?2, worktree = COALESCE(?3, worktree),
                              updated_at = ?4 WHERE id = ?1",
            params![id.as_str(), agent.as_str(), worktree, divan_core::now_ms()],
        )?;
        if n == 0 {
            return Err(DbError::NotFound(format!("task {id}")));
        }
        Ok(())
    }

    fn transition(&self, id: &TaskId, to: TaskState, reason: TransitionReason) -> DbResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;

        // Read current state under the tx.
        let from: TaskState = {
            let s: String = tx
                .query_row(
                    "SELECT state FROM tasks WHERE id = ?1",
                    [id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|_| DbError::NotFound(format!("task {id}")))?;
            serde_json::from_value(serde_json::Value::String(s))
                .map_err(|e| DbError::Integrity(format!("bad state for {id}: {e}")))?
        };

        if !from.can_transition_to(to) {
            return Err(DbError::Domain(DivanError::IllegalTransition { from, to }));
        }

        let ts = divan_core::now_ms();
        tx.execute(
            "UPDATE tasks SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.as_str(), to.as_str(), ts],
        )?;
        // task_transition trace event in the SAME transaction (impl plan §F1.2).
        let trace_id: String = tx.query_row(
            "SELECT trace_id FROM tasks WHERE id = ?1",
            [id.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO trace_events (trace_id, event, data, ts) VALUES (?1,?2,?3,?4)",
            params![
                trace_id,
                TraceEventKind::TaskTransition.as_str(),
                serde_json::json!({
                    "task_id": id.as_str(),
                    "from": from.as_str(),
                    "to": to.as_str(),
                    "reason": reason,
                })
                .to_string(),
                ts,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_store::TraceStore;
    use divan_core::TraceId;

    fn task(id: &str, state: TaskState) -> Task {
        Task {
            id: TaskId::new(id),
            parent_id: None,
            kind: TaskKind::Implement,
            title: format!("task {id}"),
            spec_ref: None,
            state,
            assignee: None,
            worktree: None,
            max_runtime_secs: None,
            trace_id: TraceId::new(format!("trace-{id}")),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn create_emits_task_created_trace() {
        let db = Db::open_in_memory().unwrap();
        db.create_task(&task("t1", TaskState::Open)).unwrap();
        let tl = db.timeline(&TraceId::new("trace-t1")).unwrap();
        assert!(tl.iter().any(|e| e.event == TraceEventKind::TaskCreated));
    }

    #[test]
    fn legal_transition_updates_state_and_traces_atomically() {
        let db = Db::open_in_memory().unwrap();
        db.create_task(&task("t1", TaskState::Open)).unwrap();
        db.transition(
            &TaskId::new("t1"),
            TaskState::Claimed,
            TransitionReason::Other("claim".into()),
        )
        .unwrap();
        assert_eq!(
            db.get_task(&TaskId::new("t1")).unwrap().unwrap().state,
            TaskState::Claimed
        );
        let tl = db.timeline(&TraceId::new("trace-t1")).unwrap();
        let trans: Vec<_> = tl
            .iter()
            .filter(|e| e.event == TraceEventKind::TaskTransition)
            .collect();
        assert_eq!(trans.len(), 1);
        assert_eq!(trans[0].data.as_ref().unwrap()["to"], "claimed");
    }

    #[test]
    fn illegal_transition_rejected_and_no_state_change() {
        let db = Db::open_in_memory().unwrap();
        db.create_task(&task("t1", TaskState::Open)).unwrap();
        let err = db
            .transition(
                &TaskId::new("t1"),
                TaskState::Done,
                TransitionReason::Completed,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            DbError::Domain(DivanError::IllegalTransition { .. })
        ));
        // State unchanged, and no spurious transition trace written.
        assert_eq!(
            db.get_task(&TaskId::new("t1")).unwrap().unwrap().state,
            TaskState::Open
        );
        let tl = db.timeline(&TraceId::new("trace-t1")).unwrap();
        assert!(!tl.iter().any(|e| e.event == TraceEventKind::TaskTransition));
    }

    #[test]
    fn deps_satisfied_only_when_all_done() {
        let db = Db::open_in_memory().unwrap();
        db.create_task(&task("impl", TaskState::Open)).unwrap();
        db.create_task(&task("review", TaskState::Open)).unwrap();
        db.add_dep(&TaskId::new("review"), &TaskId::new("impl"))
            .unwrap();
        assert!(!db.deps_satisfied(&TaskId::new("review")).unwrap());
        // Drive impl to done.
        for s in [TaskState::Claimed, TaskState::Working, TaskState::Done] {
            db.transition(&TaskId::new("impl"), s, TransitionReason::Completed)
                .unwrap();
        }
        assert!(db.deps_satisfied(&TaskId::new("review")).unwrap());
    }

    #[test]
    fn task_with_no_deps_is_satisfied() {
        let db = Db::open_in_memory().unwrap();
        db.create_task(&task("solo", TaskState::Open)).unwrap();
        assert!(db.deps_satisfied(&TaskId::new("solo")).unwrap());
    }
}

//! Daemon singleton lock + startup reconciliation (spec §3.7, impl plan §5.8).
//!
//! v1 decision: on daemon death, surviving agent sessions are NOT re-attached
//! (spec §3.7); any task left active when a fresh daemon starts is therefore
//! orphaned and is transitioned to `failed(orphaned)`. Re-attach is v2.

use divan_core::{AgentStatus, TaskState, TransitionReason};
use divan_db::{AgentStore, Db, TaskStore};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("another divan daemon is already running (pid {0})")]
    AlreadyRunning(u32),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] divan_db::DbError),
}

/// A held singleton lock file. Removed on drop (spec §3.7 tekillik).
#[derive(Debug)]
pub struct LockFile {
    path: PathBuf,
}

impl LockFile {
    /// Acquire the lock, or fail if a live daemon already holds it. A stale lock
    /// (pid no longer alive) is taken over.
    pub fn acquire(path: &Path) -> Result<Self, LifecycleError> {
        if let Ok(existing) = std::fs::read_to_string(path) {
            if let Ok(pid) = existing.trim().parse::<u32>() {
                if pid_alive(pid) {
                    return Err(LifecycleError::AlreadyRunning(pid));
                }
                // Stale lock — previous daemon died. Take over.
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, std::process::id().to_string())?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// True if a process with `pid` is alive (POSIX `kill -0`). v1 = macOS + Linux.
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Reconcile DB state with reality at startup (spec §3.7).
///
/// Every task still in `claimed|working|review` is orphaned (its session died
/// with the previous daemon); transition it to `failed(orphaned)`. All agents
/// are marked `offline`. Returns the number of orphaned tasks.
pub fn reconcile(db: &Db) -> Result<usize, LifecycleError> {
    let mut orphaned = 0;
    for task in db.list_tasks()? {
        if matches!(
            task.state,
            TaskState::Claimed | TaskState::Working | TaskState::Review
        ) {
            db.transition(&task.id, TaskState::Failed, TransitionReason::Orphaned)?;
            orphaned += 1;
        }
    }
    for agent in db.list_agents()? {
        if agent.status != AgentStatus::Offline {
            db.set_agent_status(&agent.id, AgentStatus::Offline, None)?;
        }
    }
    Ok(orphaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{
        AgentCard, AgentId, AgentTool, Capability, DeliveryKind, Task, TaskId, TaskKind, TraceId,
    };
    use divan_db::TraceStore;

    fn task(id: &str, state: TaskState) -> Task {
        Task {
            id: TaskId::new(id),
            parent_id: None,
            kind: TaskKind::Implement,
            title: "t".into(),
            spec_ref: None,
            state,
            assignee: Some(AgentId::new("claude-1")),
            worktree: None,
            max_runtime_secs: None,
            trace_id: TraceId::new(format!("tr-{id}")),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn lock_blocks_second_acquire_while_held() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("divan.lock");
        let _held = LockFile::acquire(&p).unwrap();
        // Our own pid is alive => second acquire is refused.
        match LockFile::acquire(&p) {
            Err(LifecycleError::AlreadyRunning(pid)) => assert_eq!(pid, std::process::id()),
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
    }

    #[test]
    fn lock_released_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("divan.lock");
        {
            let _held = LockFile::acquire(&p).unwrap();
        }
        assert!(!p.exists(), "lock removed on drop");
        assert!(
            LockFile::acquire(&p).is_ok(),
            "can re-acquire after release"
        );
    }

    #[test]
    fn stale_lock_is_taken_over() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("divan.lock");
        std::fs::write(&p, "999999999").unwrap(); // implausible, dead pid
        assert!(LockFile::acquire(&p).is_ok(), "stale lock taken over");
    }

    #[test]
    fn reconcile_orphans_active_tasks_and_offlines_agents() {
        let db = Db::open_in_memory().unwrap();
        let card = AgentCard {
            id: AgentId::new("claude-1"),
            tool: AgentTool::Claude,
            display_name: None,
            capabilities: vec![Capability::Write],
            cost_class: 5,
            skills: vec![],
            delivery: vec![DeliveryKind::Hook],
            multi_turn: true,
            status: AgentStatus::Busy,
            session_id: Some("dead-session".into()),
            registered_at: 0,
        };
        db.upsert_agent(&card).unwrap();
        db.create_task(&task("working", TaskState::Open)).unwrap();
        db.transition(
            &TaskId::new("working"),
            TaskState::Claimed,
            TransitionReason::Other("c".into()),
        )
        .unwrap();
        db.transition(
            &TaskId::new("working"),
            TaskState::Working,
            TransitionReason::Other("w".into()),
        )
        .unwrap();
        db.create_task(&task("done", TaskState::Open)).unwrap();
        db.transition(
            &TaskId::new("done"),
            TaskState::Claimed,
            TransitionReason::Other("c".into()),
        )
        .unwrap();
        db.transition(
            &TaskId::new("done"),
            TaskState::Working,
            TransitionReason::Other("w".into()),
        )
        .unwrap();
        db.transition(
            &TaskId::new("done"),
            TaskState::Done,
            TransitionReason::Completed,
        )
        .unwrap();

        let n = reconcile(&db).unwrap();
        assert_eq!(n, 1, "only the active task is orphaned");

        let working = db.get_task(&TaskId::new("working")).unwrap().unwrap();
        assert_eq!(working.state, TaskState::Failed);
        let done = db.get_task(&TaskId::new("done")).unwrap().unwrap();
        assert_eq!(done.state, TaskState::Done, "terminal task untouched");
        let agent = db.get_agent(&AgentId::new("claude-1")).unwrap().unwrap();
        assert_eq!(agent.status, AgentStatus::Offline);

        // The orphan trace carries reason=orphaned.
        let tl = db.timeline(&TraceId::new("tr-working")).unwrap();
        assert!(tl.iter().any(|e| e
            .data
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            == Some("orphaned")));
    }
}

//! Task domain types and the authoritative state machine (spec §4, §4.2).
//!
//! State transitions are validated here but **only performed by the
//! `TaskScheduler`** in an atomic DB transaction that also writes a
//! `trace_event` (AGENTS.md, spec §4.2). Adapters never mutate task state.

use crate::ids::{AgentId, TaskId, TraceId};
use serde::{Deserialize, Serialize};

/// Task kind (spec §4.1 `TaskKind`). `Analyze`/`Report` are one-shot kinds the
/// degraded `agy` adapter accepts (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Implement,
    Review,
    Test,
    Plan,
    Research,
    Analyze,
    Report,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Implement => "implement",
            TaskKind::Review => "review",
            TaskKind::Test => "test",
            TaskKind::Plan => "plan",
            TaskKind::Research => "research",
            TaskKind::Analyze => "analyze",
            TaskKind::Report => "report",
        }
    }
}

/// Task lifecycle state (spec §4 `tasks.state`, A2A-inspired).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Open,
    Claimed,
    Working,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Open => "open",
            TaskState::Claimed => "claimed",
            TaskState::Working => "working",
            TaskState::Review => "review",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }

    /// Terminal states never transition further.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed | TaskState::Cancelled
        )
    }

    /// Whether `self -> next` is a legal transition (spec §4.2).
    ///
    /// ```text
    /// open -> claimed
    /// claimed -> working
    /// working -> review
    /// review -> working          (review loop is allowed)
    /// review -> done
    /// working -> done
    /// open|claimed|working|review -> failed
    /// open|claimed|working|review -> cancelled
    /// ```
    pub fn can_transition_to(self, next: TaskState) -> bool {
        use TaskState::*;
        match (self, next) {
            (Open, Claimed) => true,
            (Claimed, Working) => true,
            (Working, Review) => true,
            (Review, Working) => true,
            (Review, Done) => true,
            (Working, Done) => true,
            // Failure / cancellation from any non-terminal state.
            (Open | Claimed | Working | Review, Failed) => true,
            (Open | Claimed | Working | Review, Cancelled) => true,
            _ => false,
        }
    }
}

/// Reason recorded with a transition into a terminal state (trace `data`,
/// spec §3.7, §4.4). Free-form reasons are also allowed via `Other`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionReason {
    /// Daemon startup reconciliation found the session dead (spec §3.7).
    Orphaned,
    /// Session watchdog exceeded `max_runtime_secs` (spec §3.7).
    Timeout,
    /// Adapter reported a fatal error (spec §3.2 taxonomy).
    Fatal,
    /// Normal completion.
    Completed,
    /// Human or flow cancellation.
    Cancelled,
    Other(String),
}

/// Task record (spec §4 `tasks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub parent_id: Option<TaskId>,
    pub kind: TaskKind,
    pub title: String,
    /// Artifact reference to the spec (K4 — pointer, not content).
    pub spec_ref: Option<String>,
    pub state: TaskState,
    pub assignee: Option<AgentId>,
    /// Worktree path, or `None` for read-only tasks (spec §4).
    pub worktree: Option<String>,
    /// `None` => use the kind-based config default (watchdog, spec §3.7).
    pub max_runtime_secs: Option<u32>,
    pub trace_id: TraceId,
    pub created_at: i64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::TaskState::*;

    #[test]
    fn happy_path_transitions_allowed() {
        assert!(Open.can_transition_to(Claimed));
        assert!(Claimed.can_transition_to(Working));
        assert!(Working.can_transition_to(Review));
        assert!(Review.can_transition_to(Done));
        assert!(Working.can_transition_to(Done));
    }

    #[test]
    fn review_loop_allowed() {
        assert!(Review.can_transition_to(Working));
    }

    #[test]
    fn failure_and_cancel_from_any_nonterminal() {
        for s in [Open, Claimed, Working, Review] {
            assert!(s.can_transition_to(Failed), "{s:?} -> failed");
            assert!(s.can_transition_to(Cancelled), "{s:?} -> cancelled");
        }
    }

    #[test]
    fn illegal_transitions_rejected() {
        assert!(!Open.can_transition_to(Working)); // must claim first
        assert!(!Open.can_transition_to(Done));
        assert!(!Claimed.can_transition_to(Review));
        assert!(!Done.can_transition_to(Working)); // terminal
        assert!(!Failed.can_transition_to(Open));
        assert!(!Cancelled.can_transition_to(Working));
    }

    #[test]
    fn terminal_states_are_terminal() {
        for s in [Done, Failed, Cancelled] {
            assert!(s.is_terminal());
            for next in [Open, Claimed, Working, Review, Done, Failed, Cancelled] {
                assert!(
                    !s.can_transition_to(next),
                    "{s:?} -> {next:?} must be illegal"
                );
            }
        }
    }
}

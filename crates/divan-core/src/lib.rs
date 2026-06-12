//! `divan-core` — shared domain types, the task state machine, and the error
//! taxonomy for the Divan orchestration hub.
//!
//! This crate has **no other workspace dependencies** (impl plan §3): it is the
//! contract every other crate compiles against. Keep it small and deterministic
//! (AGENTS.md coding rules; K2 — no LLM, no side effects here).

pub mod agent;
pub mod artifact;
pub mod error;
pub mod event;
pub mod ids;
pub mod message;
pub mod task;
pub mod trace;

// Flat re-exports for ergonomic downstream use.
pub use agent::{AgentCard, AgentStatus, AgentTool, Capability, CostClass, DeliveryKind};
pub use artifact::{shard_path, ArtifactMeta};
pub use error::{DivanError, Result};
pub use event::{AgentErrorClass, FileChangeKind, NormalizedEvent, Usage};
pub use ids::{AgentId, ArtifactRef, MessageId, SpanId, TaskId, TraceId};
pub use message::{validate_summary, Message, MessageKind, MAX_SUMMARY_LEN};
pub use task::{Task, TaskKind, TaskState, TransitionReason};
pub use trace::{TraceEvent, TraceEventKind};

/// Current unix time in milliseconds. Stores generally pass explicit
/// timestamps for determinism in tests; this is the production default.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

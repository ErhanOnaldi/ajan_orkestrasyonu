//! `divan-adapters` — the common agent-adapter trait and concrete adapters.
//!
//! Adapters are isolated modules (AGENTS.md "Adapter Development Rules"): they
//! translate a tool's native output into [`divan_core::NormalizedEvent`] and
//! never leak tool-specific logic into core orchestration. Adding a tool means
//! adding an adapter; the core is untouched (spec §3.2).
//!
//! ## Contract layering
//! - This file owns the **interface**: [`AgentAdapter`], [`SpawnCtx`],
//!   [`SessionHandle`], [`DeliveryReceipt`], [`MessageBatch`], [`AdapterError`].
//! - [`claude`] / [`codex`] own the **parsers** (`parse_line`) and process
//!   management, mapped per spikes S1/S2 §5.
//! - [`fake`] is a deterministic in-memory adapter for scheduler/integration
//!   tests (impl plan §F1.5, §10.2).

use async_trait::async_trait;
use divan_core::{AgentCard, AgentErrorClass, NormalizedEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub mod claude;
pub mod codex;
pub mod fake;

/// Context handed to an adapter when spawning a session (impl plan §6.1).
#[derive(Debug, Clone)]
pub struct SpawnCtx {
    /// Worktree the session runs in (its cwd); `None` for read-only tasks that
    /// reuse an existing diff (spec §5.6).
    pub worktree: Option<PathBuf>,
    /// Extra environment variables.
    pub env: HashMap<String, String>,
    /// Trace/span ids for correlation (K10).
    pub trace_id: String,
    pub span_id: String,
    /// Artifact references made available to the prompt (K4 pointers).
    pub artifact_refs: Vec<String>,
    /// When true the session must run without write access (review tasks):
    /// claude `--permission-mode plan`/no-write, codex `--sandbox read-only`
    /// (spikes S1/S2 §6).
    pub read_only: bool,
    /// The fully-rendered prompt text for the session.
    pub prompt: String,
    /// Hard turn cap passed to the CLI (`--max-turns` / equivalent).
    pub max_turns: Option<u32>,
}

/// A live (or completed) adapter session handle (impl plan §6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHandle {
    pub tool: divan_core::AgentTool,
    pub pid: Option<u32>,
    /// claude `session_id` / codex `thread_id` — the resume key (spikes S1/S2).
    pub session_id: Option<String>,
    pub task_id: String,
    pub started_at: i64,
}

/// Result of injecting a message batch into a session (impl plan §6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub accepted: bool,
    pub injected_at: i64,
    pub raw_status: Option<String>,
}

/// A single item in a delivered batch (K4/K5; full batching is Faz 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItem {
    pub message_id: String,
    pub kind: String,
    pub from_agent: String,
    pub summary: String,
    pub artifact_ref: Option<String>,
}

/// A turn-boundary batch injected into a session context (K5, spec §3.6/§5.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBatch {
    pub trace_id: String,
    pub task_id: String,
    pub items: Vec<BatchItem>,
}

/// Adapter-level error carrying the class that drives scheduler retry
/// (spec §3.2, §6.1). Unclassifiable failures MUST be `Fatal`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("adapter error ({class:?}): {message}")]
pub struct AdapterError {
    pub class: AgentErrorClass,
    pub message: String,
}

impl AdapterError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            class: AgentErrorClass::Fatal,
            message: message.into(),
        }
    }
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            class: AgentErrorClass::Retryable,
            message: message.into(),
        }
    }
}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

/// A boxed stream of normalized events from an observed session.
pub type EventStream = tokio::sync::mpsc::Receiver<NormalizedEvent>;

/// The common adapter trait (spec §3.2, impl plan §6.1).
///
/// The hub holds adapters as trait objects and only ever sees normalized
/// events. State transitions are the scheduler's job, never the adapter's.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// Declared capabilities + delivery paths (spec §3.2 `card`).
    fn card(&self) -> AgentCard;

    /// Start a session for `prompt` in `ctx.worktree`.
    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle>;

    /// Stream normalized events from a running session.
    async fn observe(&self, handle: &SessionHandle) -> AdapterResult<EventStream>;

    /// Inject a turn-boundary message batch (Faz 2 surface; defined now).
    async fn deliver(
        &self,
        handle: &SessionHandle,
        batch: MessageBatch,
    ) -> AdapterResult<DeliveryReceipt>;

    /// Resume a prior session. Adapters that cannot resume (spec §3.5: agy)
    /// return a `Fatal` `AdapterError` and declare this in their card.
    async fn resume(&self, session_id: &str, prompt: &str) -> AdapterResult<SessionHandle>;

    /// Terminate a session.
    async fn kill(&self, handle: &SessionHandle) -> AdapterResult<()>;
}

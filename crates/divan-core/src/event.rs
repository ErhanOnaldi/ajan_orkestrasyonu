//! Normalized adapter events and the error taxonomy (spec §3.2, §4.1).
//!
//! Every adapter maps its tool-specific output (claude `stream-json`, codex
//! `exec --json`, copilot diff-derived, …) into this single enum. The hub only
//! ever sees normalized events (spec §3.2 "Hub yalnızca normalize olay görür").
//!
//! The variant set is fixed by spec §4.1: `ToolCall`, `FileEdit`, `TurnEnd`,
//! `SessionIdle`, `SessionEnd`, `Error`. Session-start identity (claude
//! `session_id` / codex `thread_id`) is captured by the adapter into its
//! `SessionHandle`, not emitted as an event.

use serde::{Deserialize, Serialize};

/// File-change kind, native in codex (`file_change.changes[].kind`) and
/// heuristic for claude (`Write` => Added, `Edit` => Modified) — see spikes
/// S1/S2 §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

/// Token usage telemetry attached to `SessionEnd` (spec §5.3 metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
}

/// Adapter error class driving the scheduler's deterministic retry policy
/// (spec §3.2, §6.1). An adapter that cannot classify MUST assume `Fatal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentErrorClass {
    /// Rate limit, transient network/tool error, transient MCP disconnect.
    /// Scheduler retries with bounded backoff (default 2).
    Retryable,
    /// Missing auth, CLI not found, parse collapse, policy denial,
    /// unsupported resume. Scheduler transitions the task to `failed`.
    Fatal,
}

/// A normalized adapter event (spec §3.2, §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum NormalizedEvent {
    /// A tool invocation (shell command, MCP tool, built-in tool).
    ToolCall {
        name: String,
        /// Tool-specific input, kept as opaque JSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    /// A file was created/modified/deleted in the worktree.
    FileEdit { path: String, kind: FileChangeKind },
    /// Turn boundary (claude `post_turn_summary`, codex `turn.completed`).
    /// Faz 2 message batching injects at this boundary (K5).
    TurnEnd,
    /// The session went idle awaiting input. Not present in either tool's
    /// stream (spikes S1/S2 §5) — supplied by hooks / resume path in Faz 2.
    SessionIdle,
    /// The session ended.
    SessionEnd {
        ok: bool,
        #[serde(default)]
        usage: Usage,
        /// The agent's final text (claude `result.result`, codex last
        /// `agent_message`). This is the review/implement result the hub turns
        /// into an artifact (spec §5.2 "Review sonucu artifact olur").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_text: Option<String>,
    },
    /// An error, carrying its class for the retry decision.
    Error {
        class: AgentErrorClass,
        message: String,
    },
}

impl NormalizedEvent {
    /// Convenience constructor for a fatal error.
    pub fn fatal(message: impl Into<String>) -> Self {
        NormalizedEvent::Error {
            class: AgentErrorClass::Fatal,
            message: message.into(),
        }
    }

    /// Convenience constructor for a retryable error.
    pub fn retryable(message: impl Into<String>) -> Self {
        NormalizedEvent::Error {
            class: AgentErrorClass::Retryable,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_tagged_json() {
        let ev = NormalizedEvent::ToolCall {
            name: "Write".into(),
            input: None,
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"event\":\"tool_call\""));
        assert!(s.contains("\"name\":\"Write\""));
    }

    #[test]
    fn error_roundtrip_preserves_class() {
        let ev = NormalizedEvent::fatal("cli not found");
        let s = serde_json::to_string(&ev).unwrap();
        let back: NormalizedEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
        match back {
            NormalizedEvent::Error { class, .. } => assert_eq!(class, AgentErrorClass::Fatal),
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn session_end_default_usage() {
        let ev = NormalizedEvent::SessionEnd {
            ok: true,
            usage: Usage::default(),
            final_text: Some("done".into()),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: NormalizedEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }
}

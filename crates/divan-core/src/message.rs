//! Pointer-based message domain types (K4, spec §4 `messages`).
//!
//! Messages carry a pointer + a structured summary of at most
//! [`MAX_SUMMARY_LEN`] characters. Heavy content lives in the artifact store
//! and is referenced via `artifact_ref` (K4, AGENTS.md "Message Rules").

use crate::error::{DivanError, Result};
use crate::ids::{AgentId, ArtifactRef, MessageId, TaskId, TraceId};
use serde::{Deserialize, Serialize};

/// Maximum message summary length, enforced at both the API and DB layers
/// (spec §4 `CHECK(length(summary) <= 400)`, AGENTS.md "max summary length = 400").
pub const MAX_SUMMARY_LEN: usize = 400;

/// Message kind (spec §4 `messages.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Handoff,
    ReviewDone,
    Question,
    Status,
    Alert,
}

impl MessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Handoff => "handoff",
            MessageKind::ReviewDone => "review_done",
            MessageKind::Question => "question",
            MessageKind::Status => "status",
            MessageKind::Alert => "alert",
        }
    }
}

/// Validate a message/artifact summary: non-empty and `<= MAX_SUMMARY_LEN`
/// characters (counted by Unicode scalar values, matching SQLite `length()`
/// semantics closely enough for the contract).
pub fn validate_summary(summary: &str) -> Result<()> {
    if summary.is_empty() {
        return Err(DivanError::InvalidSummary {
            reason: "summary must not be empty".into(),
        });
    }
    let len = summary.chars().count();
    if len > MAX_SUMMARY_LEN {
        return Err(DivanError::InvalidSummary {
            reason: format!("summary length {len} exceeds {MAX_SUMMARY_LEN}"),
        });
    }
    Ok(())
}

/// Message record (spec §4 `messages`).
///
/// Fan-out rule (P0.2 / Option A): a broadcast source row keeps `to_agent =
/// None` and never enters the delivery queue; per-target copies set `to_agent`
/// and link back via `origin_message_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    /// Set on a fan-out copy; points at the source broadcast row (P0.2).
    pub origin_message_id: Option<MessageId>,
    pub from_agent: AgentId,
    /// `None` only on the source broadcast row.
    pub to_agent: Option<AgentId>,
    pub kind: MessageKind,
    pub summary: String,
    /// Small structured JSON payload (never heavy content, K4).
    pub payload: Option<serde_json::Value>,
    pub artifact_ref: Option<ArtifactRef>,
    pub task_id: Option<TaskId>,
    pub trace_id: Option<TraceId>,
    pub created_at: i64,
    /// `None` => queued (batching, K5); written after a delivery receipt.
    pub delivered_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_summary_rejected() {
        assert!(validate_summary("").is_err());
    }

    #[test]
    fn max_len_boundary() {
        let ok = "x".repeat(MAX_SUMMARY_LEN);
        assert!(validate_summary(&ok).is_ok());
        let too_long = "x".repeat(MAX_SUMMARY_LEN + 1);
        assert!(validate_summary(&too_long).is_err());
    }

    #[test]
    fn counts_unicode_scalars_not_bytes() {
        // 400 multi-byte chars must pass even though byte length > 400.
        let s = "é".repeat(MAX_SUMMARY_LEN);
        assert!(s.len() > MAX_SUMMARY_LEN); // bytes
        assert!(validate_summary(&s).is_ok()); // chars
    }

    #[test]
    fn kind_snake_case_json() {
        assert_eq!(
            serde_json::to_string(&MessageKind::ReviewDone).unwrap(),
            "\"review_done\""
        );
    }
}

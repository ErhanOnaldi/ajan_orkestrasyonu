//! Trace/observability domain types (K10, spec §4 `trace_events`, §5.3).
//!
//! Every critical behavior emits a trace event in the same DB transaction as
//! the state change (AGENTS.md "Trace everything", impl plan §1.7).

use crate::ids::{AgentId, SpanId, TraceId};
use serde::{Deserialize, Serialize};

/// Trace event kind (impl plan §5.7 event names). Exhaustive so the compiler
/// flags any new behavior that forgets to emit a trace event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    TaskCreated,
    TaskTransition,
    RouterDecision,
    AgentRegistered,
    Spawn,
    SessionEvent,
    ToolCall,
    FileEdit,
    ArtifactPublished,
    MsgSent,
    MsgDelivered,
    PolicyDenied,
    WorktreeCreated,
    WorktreeDiffed,
    WorktreeMergeRequested,
    WorktreeMerged,
    Error,
}

impl TraceEventKind {
    pub fn as_str(self) -> &'static str {
        use TraceEventKind::*;
        match self {
            TaskCreated => "task_created",
            TaskTransition => "task_transition",
            RouterDecision => "router_decision",
            AgentRegistered => "agent_registered",
            Spawn => "spawn",
            SessionEvent => "session_event",
            ToolCall => "tool_call",
            FileEdit => "file_edit",
            ArtifactPublished => "artifact_published",
            MsgSent => "msg_sent",
            MsgDelivered => "msg_delivered",
            PolicyDenied => "policy_denied",
            WorktreeCreated => "worktree_created",
            WorktreeDiffed => "worktree_diffed",
            WorktreeMergeRequested => "worktree_merge_requested",
            WorktreeMerged => "worktree_merged",
            Error => "error",
        }
    }
}

/// A single trace event row (spec §4 `trace_events`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub trace_id: TraceId,
    pub span_id: Option<SpanId>,
    pub parent_span: Option<SpanId>,
    pub agent_id: Option<AgentId>,
    pub event: TraceEventKind,
    /// Structured JSON detail: transition reason, `orphaned`/`timeout`, retry
    /// count, policy reason, etc. (spec §4.4).
    pub data: Option<serde_json::Value>,
    pub ts: i64,
}

impl TraceEvent {
    /// Construct a trace event with structured data.
    pub fn new(trace_id: TraceId, event: TraceEventKind, ts: i64) -> Self {
        Self {
            trace_id,
            span_id: None,
            parent_span: None,
            agent_id: None,
            event,
            data: None,
            ts,
        }
    }

    pub fn with_agent(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn with_span(mut self, span_id: SpanId) -> Self {
        self.span_id = Some(span_id);
        self
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_snake_case() {
        assert_eq!(TraceEventKind::TaskTransition.as_str(), "task_transition");
        assert_eq!(
            serde_json::to_string(&TraceEventKind::PolicyDenied).unwrap(),
            "\"policy_denied\""
        );
    }

    #[test]
    fn builder_sets_fields() {
        let ev = TraceEvent::new(TraceId::new("t1"), TraceEventKind::Spawn, 123)
            .with_agent(AgentId::new("claude-1"))
            .with_data(serde_json::json!({"reason": "orphaned"}));
        assert_eq!(ev.agent_id, Some(AgentId::new("claude-1")));
        assert_eq!(ev.data.unwrap()["reason"], "orphaned");
    }
}

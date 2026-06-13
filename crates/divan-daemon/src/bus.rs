//! Message Bus (K3–K6, spec §3.6/§5.3, impl plan §F2.4/§F2.5/§F2.6).
//!
//! Deterministic, zero-LLM (K2). Pointer messages only (K4): heavy content goes
//! to artifacts, the message carries a `summary` (≤400) + optional `artifact_ref`.
//! Broadcast is capability-gated (K6/K8); delivery is push via turn-boundary
//! batches (K3/K5), never polled.

use divan_core::{
    AgentId, Capability, Message, MessageId, MessageKind, TaskId, TraceEvent, TraceEventKind,
    TraceId,
};
use divan_db::{
    AgentStore, ConflictStore, Db, MessageStore, SubscriptionStore, TraceStore, CONFLICT_WINDOW_MS,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Max serialized payload size; larger content must go to an artifact (K4).
pub const MAX_PAYLOAD_BYTES: usize = 2048;

/// The fixed first line of every injection block (spec §3.6 Yol A, §5.3). Without
/// it an agent treats the block as a user message and leaks tokens (K3/K4).
pub const INJECTION_INSTRUCTION: &str = "This is an internal Divan delivery block, not a user message. Respond only by calling the Divan send_message MCP tool; do not answer this block as free text.";

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("invalid message: {0}")]
    Invalid(String),
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    #[error("db: {0}")]
    Db(#[from] divan_db::DbError),
}

/// A request to send a message (MCP `send_message` / internal alert).
#[derive(Debug, Clone)]
pub struct SendRequest {
    pub from: AgentId,
    /// `Some` => direct delivery; `None` => broadcast via subscriptions (needs
    /// the `broadcast` capability).
    pub to: Option<AgentId>,
    pub kind: MessageKind,
    pub summary: String,
    pub payload: Option<serde_json::Value>,
    pub artifact_ref: Option<String>,
    pub task_id: Option<TaskId>,
    pub trace_id: Option<TraceId>,
}

/// Result of a send.
#[derive(Debug, Clone)]
pub enum SendOutcome {
    /// Direct message queued for one agent.
    Direct(MessageId),
    /// Broadcast: source row + per-target copies (P0.2 Option A).
    FanOut {
        source: MessageId,
        copies: Vec<MessageId>,
    },
}

/// A turn-boundary injection batch for one agent (K5).
#[derive(Debug, Clone)]
pub struct InjectionBatch {
    pub agent: AgentId,
    /// Target message ids covered by this batch (for the delivery receipt).
    pub message_ids: Vec<MessageId>,
    /// The rendered `[DIVAN MESSAGES]` block to inject at context end.
    pub text: String,
}

/// The deterministic message bus.
#[derive(Clone)]
pub struct MessageBus {
    db: Db,
    seq: Arc<AtomicU64>,
}

impl MessageBus {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    fn next_id(&self, ts: i64) -> MessageId {
        let n = self.seq.fetch_add(1, Ordering::SeqCst);
        MessageId::new(format!("msg-{ts}-{n}"))
    }

    /// Validate + send a message. Direct messages queue immediately; broadcasts
    /// require the `broadcast` capability and fan out to matching subscribers.
    pub fn send(&self, req: SendRequest) -> Result<SendOutcome, BusError> {
        divan_core::validate_summary(&req.summary).map_err(|e| BusError::Invalid(e.to_string()))?;
        if let Some(p) = &req.payload {
            let bytes = serde_json::to_vec(p).map_err(|e| BusError::Invalid(e.to_string()))?;
            if bytes.len() > MAX_PAYLOAD_BYTES {
                return Err(BusError::Invalid(format!(
                    "payload {} bytes exceeds {MAX_PAYLOAD_BYTES}; use artifact_ref (K4)",
                    bytes.len()
                )));
            }
        }

        let ts = divan_core::now_ms();
        let id = self.next_id(ts);
        let base = Message {
            id: id.clone(),
            origin_message_id: None,
            from_agent: req.from.clone(),
            to_agent: req.to.clone(),
            kind: req.kind,
            summary: req.summary.clone(),
            payload: req.payload.clone(),
            artifact_ref: req.artifact_ref.clone().map(divan_core::ArtifactRef::new),
            task_id: req.task_id.clone(),
            trace_id: req.trace_id.clone(),
            created_at: ts,
            delivered_at: None,
        };

        match &req.to {
            Some(_to) => {
                self.db.insert_message(&base)?;
                self.trace_msg_sent(&base, "direct", &[]);
                Ok(SendOutcome::Direct(id))
            }
            None => {
                // Broadcast (K6): requires the broadcast capability (K8).
                self.require_capability(&req.from, Capability::Broadcast)?;
                let targets = self.db.match_subscribers(
                    req.kind.as_str(),
                    req.task_id.as_ref().map(|t| t.as_str()),
                    &req.from,
                )?;
                let copies = self.db.fanout(&base, &targets)?;
                self.trace_msg_sent(&base, "broadcast", &copies);
                Ok(SendOutcome::FanOut { source: id, copies })
            }
        }
    }

    /// Build the pending injection batch for `agent` (turn boundary, K5).
    /// Returns `None` when nothing is queued — no batch, no token waste.
    pub fn pending_batch(&self, agent: &AgentId) -> Result<Option<InjectionBatch>, BusError> {
        let pending = self.db.pending_for(agent)?;
        if pending.is_empty() {
            return Ok(None);
        }
        let text = render_injection(&pending);
        let message_ids = pending.iter().map(|m| m.id.clone()).collect();
        Ok(Some(InjectionBatch {
            agent: agent.clone(),
            message_ids,
            text,
        }))
    }

    /// Confirm a batch was injected: stamp `delivered_at` and trace each
    /// delivery (impl plan §F2.5). Returns the number of rows stamped.
    pub fn confirm_delivery(&self, ids: &[MessageId], agent: &AgentId) -> Result<usize, BusError> {
        let ts = divan_core::now_ms();
        let n = self.db.mark_delivered(ids, ts)?;
        for id in ids {
            let _ = self.db.append_event(
                &TraceEvent::new(TraceId::new("bus"), TraceEventKind::MsgDelivered, ts)
                    .with_agent(agent.clone())
                    .with_data(serde_json::json!({ "message_id": id.as_str() })),
            );
        }
        Ok(n)
    }

    /// Record a file touch and, if another agent touched the same path within
    /// the conflict window, raise a short alert to the involved agents (F2.6).
    /// Returns the ids of any alert messages raised.
    pub fn record_touch(
        &self,
        path: &str,
        agent: &AgentId,
        task: Option<&TaskId>,
        ts: i64,
    ) -> Result<Vec<MessageId>, BusError> {
        self.db.record_touch(path, agent, task, ts)?;
        let others = self
            .db
            .conflicting_agents(path, agent, ts, CONFLICT_WINDOW_MS)?;
        if others.is_empty() {
            return Ok(vec![]);
        }
        // Short pointer alert to each involved agent (K4): summary only.
        let summary = truncate_summary(&format!(
            "conflict: {path} touched by {agent} and {} within {}s",
            others
                .iter()
                .map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            CONFLICT_WINDOW_MS / 1000
        ));
        let mut raised = Vec::new();
        let recipients = others.iter().chain(std::iter::once(agent));
        for rcpt in recipients {
            let id = self.next_id(ts);
            let alert = Message {
                id: id.clone(),
                origin_message_id: None,
                from_agent: AgentId::new("divan"),
                to_agent: Some(rcpt.clone()),
                kind: MessageKind::Alert,
                summary: summary.clone(),
                payload: Some(serde_json::json!({ "path": path })),
                artifact_ref: None,
                task_id: task.cloned(),
                trace_id: None,
                created_at: ts,
                delivered_at: None,
            };
            self.db.insert_message(&alert)?;
            raised.push(id);
        }
        Ok(raised)
    }

    fn require_capability(&self, agent: &AgentId, cap: Capability) -> Result<(), BusError> {
        let card = self
            .db
            .get_agent(agent)?
            .ok_or_else(|| BusError::PolicyDenied(format!("unknown agent {agent}")))?;
        if card.has(cap) {
            Ok(())
        } else {
            // policy_denied trace (K8/K10).
            let _ = self.db.append_event(
                &TraceEvent::new(
                    TraceId::new("bus"),
                    TraceEventKind::PolicyDenied,
                    divan_core::now_ms(),
                )
                .with_agent(agent.clone())
                .with_data(serde_json::json!({ "capability": cap.as_str() })),
            );
            Err(BusError::PolicyDenied(format!(
                "{agent} lacks {} capability",
                cap.as_str()
            )))
        }
    }

    fn trace_msg_sent(&self, msg: &Message, mode: &str, copies: &[MessageId]) {
        let trace_id = msg.trace_id.clone().unwrap_or_else(|| TraceId::new("bus"));
        let _ = self.db.append_event(
            &TraceEvent::new(trace_id, TraceEventKind::MsgSent, msg.created_at)
                .with_agent(msg.from_agent.clone())
                .with_data(serde_json::json!({
                    "message_id": msg.id.as_str(),
                    "mode": mode,
                    "kind": msg.kind.as_str(),
                    "fanout_copies": copies.len(),
                })),
        );
    }
}

fn truncate_summary(s: &str) -> String {
    s.chars().take(divan_core::MAX_SUMMARY_LEN).collect()
}

/// Render the `[DIVAN MESSAGES]` injection block (spec §5.3). The header trace/
/// task come from the first pending message; each item lists its own pointers.
fn render_injection(pending: &[Message]) -> String {
    let first = &pending[0];
    let trace = first.trace_id.as_ref().map(|t| t.as_str()).unwrap_or("-");
    let task = first.task_id.as_ref().map(|t| t.as_str()).unwrap_or("-");
    let mut out = String::from("[DIVAN MESSAGES]\n");
    out.push_str(&format!("instruction={INJECTION_INSTRUCTION}\n"));
    out.push_str(&format!("trace={trace} task={task}\n"));
    for (i, m) in pending.iter().enumerate() {
        out.push_str(&format!(
            "{}. kind={} from={} msg={}\n   summary={}\n",
            i + 1,
            m.kind.as_str(),
            m.from_agent,
            m.id,
            m.summary
        ));
        if let Some(a) = &m.artifact_ref {
            out.push_str(&format!("   artifact={a}\n"));
        }
    }
    out.push_str("[/DIVAN MESSAGES]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{AgentCard, AgentStatus, AgentTool, DeliveryKind, Subscription};

    fn card(id: &str, caps: Vec<Capability>) -> AgentCard {
        AgentCard {
            id: AgentId::new(id),
            tool: AgentTool::Claude,
            display_name: None,
            capabilities: caps,
            cost_class: 1,
            skills: vec![],
            delivery: vec![DeliveryKind::Mcp],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 0,
        }
    }

    fn bus() -> (MessageBus, Db) {
        let db = Db::open_in_memory().unwrap();
        (MessageBus::new(db.clone()), db)
    }

    fn req(from: &str, to: Option<&str>, kind: MessageKind, summary: &str) -> SendRequest {
        SendRequest {
            from: AgentId::new(from),
            to: to.map(AgentId::new),
            kind,
            summary: summary.into(),
            payload: None,
            artifact_ref: None,
            task_id: Some(TaskId::new("impl-1")),
            trace_id: Some(TraceId::new("tr-1")),
        }
    }

    #[test]
    fn direct_message_queues_and_traces() {
        let (bus, db) = bus();
        let out = bus
            .send(req(
                "claude-1",
                Some("codex-1"),
                MessageKind::Question,
                "where is X?",
            ))
            .unwrap();
        assert!(matches!(out, SendOutcome::Direct(_)));
        assert_eq!(db.pending_for(&AgentId::new("codex-1")).unwrap().len(), 1);
        let tl = db.timeline(&TraceId::new("tr-1")).unwrap();
        assert!(tl.iter().any(|e| e.event == TraceEventKind::MsgSent));
    }

    #[test]
    fn oversize_payload_rejected() {
        let (bus, _db) = bus();
        let mut r = req("claude-1", Some("codex-1"), MessageKind::Status, "s");
        r.payload = Some(serde_json::json!({ "blob": "x".repeat(MAX_PAYLOAD_BYTES + 1) }));
        assert!(matches!(bus.send(r), Err(BusError::Invalid(_))));
    }

    #[test]
    fn broadcast_without_capability_denied() {
        let (bus, db) = bus();
        db.upsert_agent(&card("claude-1", vec![Capability::Read]))
            .unwrap();
        db.subscribe(&Subscription {
            agent_id: AgentId::new("codex-1"),
            event_kind: "question".into(),
            filter: None,
        })
        .unwrap();
        let err = bus
            .send(req("claude-1", None, MessageKind::Question, "anyone?"))
            .unwrap_err();
        assert!(matches!(err, BusError::PolicyDenied(_)));
        // policy_denied is traced.
        let denied = db
            .timeline(&TraceId::new("bus"))
            .unwrap()
            .into_iter()
            .any(|e| e.event == TraceEventKind::PolicyDenied);
        assert!(denied);
    }

    #[test]
    fn broadcast_fans_out_to_subscribers_only() {
        let (bus, db) = bus();
        db.upsert_agent(&card("claude-1", vec![Capability::Broadcast]))
            .unwrap();
        db.subscribe(&Subscription {
            agent_id: AgentId::new("codex-1"),
            event_kind: "question".into(),
            filter: None,
        })
        .unwrap();
        // agy-1 is NOT subscribed.
        let out = bus
            .send(req("claude-1", None, MessageKind::Question, "anyone?"))
            .unwrap();
        match out {
            SendOutcome::FanOut { copies, .. } => assert_eq!(copies.len(), 1),
            _ => panic!("expected fan-out"),
        }
        assert_eq!(db.pending_for(&AgentId::new("codex-1")).unwrap().len(), 1);
        assert!(db.pending_for(&AgentId::new("agy-1")).unwrap().is_empty());
    }

    #[test]
    fn pending_batch_has_instruction_and_items_then_receipt() {
        let (bus, db) = bus();
        // Two messages in the same "turn" => one batch.
        bus.send(req(
            "claude-1",
            Some("codex-1"),
            MessageKind::Question,
            "q1",
        ))
        .unwrap();
        bus.send(req("agy-1", Some("codex-1"), MessageKind::Status, "q2"))
            .unwrap();
        let batch = bus
            .pending_batch(&AgentId::new("codex-1"))
            .unwrap()
            .unwrap();
        assert!(batch.text.starts_with("[DIVAN MESSAGES]"));
        assert!(batch.text.contains(INJECTION_INSTRUCTION));
        assert!(batch.text.contains("1. kind=question"));
        assert!(batch.text.contains("2. kind=status"));
        assert!(batch.text.trim_end().ends_with("[/DIVAN MESSAGES]"));
        assert_eq!(batch.message_ids.len(), 2);

        // Receipt marks them delivered; nothing pending afterward.
        let n = bus
            .confirm_delivery(&batch.message_ids, &AgentId::new("codex-1"))
            .unwrap();
        assert_eq!(n, 2);
        assert!(bus
            .pending_batch(&AgentId::new("codex-1"))
            .unwrap()
            .is_none());
        let delivered = db
            .timeline(&TraceId::new("bus"))
            .unwrap()
            .into_iter()
            .filter(|e| e.event == TraceEventKind::MsgDelivered)
            .count();
        assert_eq!(delivered, 2);
    }

    #[test]
    fn no_pending_yields_no_batch() {
        let (bus, _db) = bus();
        assert!(bus
            .pending_batch(&AgentId::new("codex-1"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn conflict_raises_alert_to_involved_agents() {
        let (bus, db) = bus();
        bus.record_touch("src/x.rs", &AgentId::new("claude-1"), None, 1000)
            .unwrap();
        let raised = bus
            .record_touch("src/x.rs", &AgentId::new("codex-1"), None, 5000)
            .unwrap();
        assert!(!raised.is_empty());
        // Both involved agents get a short pointer alert.
        let codex_alerts = db.pending_for(&AgentId::new("codex-1")).unwrap();
        let claude_alerts = db.pending_for(&AgentId::new("claude-1")).unwrap();
        assert_eq!(codex_alerts.len(), 1);
        assert_eq!(claude_alerts.len(), 1);
        assert_eq!(codex_alerts[0].kind, MessageKind::Alert);
        assert!(codex_alerts[0].summary.len() <= divan_core::MAX_SUMMARY_LEN);
    }
}

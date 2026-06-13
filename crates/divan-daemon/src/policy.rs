//! Phase 2 policy chokepoint (impl plan §F2.3: "all tool handlers pass through
//! policy control"). Every MCP tool handler routes through [`check`].
//!
//! Phase 2 enforces only the capabilities that are meaningful now — broadcast
//! (gated in the [`crate::bus::MessageBus`] where the sender is known), `delegate`
//! for delegating tasks, and `read` for subscribing. The remaining actions pass
//! through with `Allow`; their capability rules land with the full matrix in
//! Phase 3 (spec §5.1, F3.1). Denials emit a `policy_denied` trace (K8/K10).

use divan_core::{AgentId, Capability, TraceEvent, TraceEventKind, TraceId};
use divan_db::{AgentStore, Db, TraceStore};

/// A policy-relevant action behind an MCP tool (impl plan §5.4 action model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SendDirect,
    Broadcast,
    Delegate,
    Claim,
    Complete,
    Publish,
    GetArtifact,
    Subscribe,
    ListAgents,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::SendDirect => "send_direct",
            Action::Broadcast => "broadcast",
            Action::Delegate => "delegate",
            Action::Claim => "claim",
            Action::Complete => "complete",
            Action::Publish => "publish",
            Action::GetArtifact => "get_artifact",
            Action::Subscribe => "subscribe",
            Action::ListAgents => "list_agents",
        }
    }

    /// Capability required to perform this action in Phase 2. `None` = allowed
    /// for now; the full per-action matrix (spec §5.1) lands in Phase 3.
    pub fn required_cap(self) -> Option<Capability> {
        match self {
            // Broadcast is enforced in the bus (it holds the sender); listed here
            // for completeness so the action model is exhaustive.
            Action::Broadcast => Some(Capability::Broadcast),
            Action::Delegate => Some(Capability::Delegate),
            Action::Subscribe => Some(Capability::Read),
            Action::SendDirect
            | Action::Claim
            | Action::Complete
            | Action::Publish
            | Action::GetArtifact
            | Action::ListAgents => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("policy denied: {agent} lacks {} capability for {}", .cap.as_str(), .action.as_str())]
pub struct PolicyDenied {
    pub agent: String,
    pub action: Action,
    pub cap: Capability,
}

/// Route an action through the policy chokepoint. Actions with no Phase 2
/// capability requirement are allowed (and still "pass through" here, satisfying
/// F2.3). Gated actions require `agent` to be registered with the capability;
/// denials are traced.
pub fn check(db: &Db, agent: Option<&AgentId>, action: Action) -> Result<(), PolicyDenied> {
    let Some(cap) = action.required_cap() else {
        return Ok(());
    };
    let allowed = match agent {
        Some(a) => db
            .get_agent(a)
            .ok()
            .flatten()
            .map(|c| c.has(cap))
            .unwrap_or(false),
        None => false,
    };
    if allowed {
        return Ok(());
    }
    let who = agent.map(|a| a.to_string()).unwrap_or_default();
    let _ = db.append_event(
        &TraceEvent::new(
            TraceId::new("policy"),
            TraceEventKind::PolicyDenied,
            divan_core::now_ms(),
        )
        .with_data(serde_json::json!({
            "agent": who, "action": action.as_str(), "capability": cap.as_str(),
        })),
    );
    Err(PolicyDenied {
        agent: who,
        action,
        cap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{AgentCard, AgentStatus, AgentTool, DeliveryKind};

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

    #[test]
    fn ungated_actions_pass_through() {
        let db = Db::open_in_memory().unwrap();
        assert!(check(&db, None, Action::Publish).is_ok());
        assert!(check(&db, Some(&AgentId::new("x")), Action::Claim).is_ok());
    }

    #[test]
    fn delegate_requires_delegate_capability() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("codex-1", vec![Capability::Read]))
            .unwrap();
        db.upsert_agent(&card(
            "claude-1",
            vec![Capability::Read, Capability::Delegate],
        ))
        .unwrap();
        assert!(check(&db, Some(&AgentId::new("codex-1")), Action::Delegate).is_err());
        assert!(check(&db, Some(&AgentId::new("claude-1")), Action::Delegate).is_ok());
    }

    #[test]
    fn subscribe_requires_read_and_denial_is_traced() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("noread", vec![])).unwrap();
        assert!(check(&db, Some(&AgentId::new("noread")), Action::Subscribe).is_err());
        let traced = db
            .timeline(&TraceId::new("policy"))
            .unwrap()
            .into_iter()
            .any(|e| e.event == TraceEventKind::PolicyDenied);
        assert!(traced);
    }

    #[test]
    fn unknown_agent_denied_for_gated_action() {
        let db = Db::open_in_memory().unwrap();
        assert!(check(&db, Some(&AgentId::new("ghost")), Action::Delegate).is_err());
    }
}

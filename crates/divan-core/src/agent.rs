//! Agent registry domain types (spec §4 `agents`, §4.1, K8).

use crate::ids::AgentId;
use serde::{Deserialize, Serialize};

/// Supported coding-agent CLI tools (spec §4.1 `AgentTool`).
///
/// The hub treats each tool's top-level agent as a single opaque peer; inner
/// subagents are never addressed (AGENTS.md "kapalı kutu").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTool {
    Claude,
    Codex,
    Copilot,
    Agy,
    OpenCode,
}

impl AgentTool {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentTool::Claude => "claude",
            AgentTool::Codex => "codex",
            AgentTool::Copilot => "copilot",
            AgentTool::Agy => "agy",
            AgentTool::OpenCode => "opencode",
        }
    }
}

/// Capability-based permission set (K8, spec §4.1 `Capability`).
///
/// Default is the narrowest set; capabilities can only be widened by a human
/// at runtime, never by the agent itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Read,
    Write,
    Spawn,
    Kill,
    Delegate,
    Broadcast,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Read => "read",
            Capability::Write => "write",
            Capability::Spawn => "spawn",
            Capability::Kill => "kill",
            Capability::Delegate => "delegate",
            Capability::Broadcast => "broadcast",
        }
    }
}

/// Delivery path an adapter declares it supports, in priority order
/// (spec §3.6: hook > MCP push > resume).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryKind {
    Hook,
    Mcp,
    Resume,
}

/// Agent runtime status (spec §4 `agents.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Busy,
    Offline,
}

/// Cost class 1 (cheap) .. 5 (expensive) (spec §4 `agents.cost_class`, K9).
pub type CostClass = u8;

/// Agent registration record (A2A AgentCard adaptation, spec §4 `agents`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub id: AgentId,
    pub tool: AgentTool,
    pub display_name: Option<String>,
    pub capabilities: Vec<Capability>,
    pub cost_class: CostClass,
    pub skills: Vec<String>,
    pub delivery: Vec<DeliveryKind>,
    /// `false` => single-shot only; router excludes from multi-turn tasks
    /// (spec §3.5: agy = false).
    pub multi_turn: bool,
    pub status: AgentStatus,
    pub session_id: Option<String>,
    pub registered_at: i64,
}

impl AgentCard {
    /// True if the agent was registered with the given capability.
    pub fn has(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_lowercase_json() {
        let json = serde_json::to_string(&Capability::Delegate).unwrap();
        assert_eq!(json, "\"delegate\"");
    }

    #[test]
    fn tool_str_matches_serde() {
        assert_eq!(AgentTool::OpenCode.as_str(), "opencode");
        assert_eq!(
            serde_json::to_string(&AgentTool::OpenCode).unwrap(),
            "\"opencode\""
        );
    }

    #[test]
    fn has_capability() {
        let card = AgentCard {
            id: AgentId::new("claude-1"),
            tool: AgentTool::Claude,
            display_name: None,
            capabilities: vec![Capability::Read, Capability::Write],
            cost_class: 5,
            skills: vec!["implement".into()],
            delivery: vec![DeliveryKind::Hook],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 0,
        };
        assert!(card.has(Capability::Write));
        assert!(!card.has(Capability::Broadcast));
    }
}

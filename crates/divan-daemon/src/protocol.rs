//! JSON-RPC IPC protocol shared by the daemon and the `divan` CLI (impl plan
//! §F1.3). Framing is one newline-delimited JSON request per connection, with
//! one JSON response. Unix socket only (v1 = macOS + Linux, P0.4).

use serde::{Deserialize, Serialize};

/// A request from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl RpcRequest {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }
}

/// A response from the daemon to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub result: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RpcResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result,
            error: None,
        }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: serde_json::Value::Null,
            error: Some(msg.into()),
        }
    }
}

/// Parameters for `run` (the write-review flow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunParams {
    pub title: String,
    pub spec: String,
    pub repo: String,
    #[serde(default = "default_writer")]
    pub writer: String,
    #[serde(default = "default_reviewer")]
    pub reviewer: String,
}

fn default_writer() -> String {
    "claude-1".into()
}
fn default_reviewer() -> String {
    "codex-1".into()
}

/// RPC method names (avoids stringly-typed drift between daemon, CLI, MCP, hooks).
pub mod method {
    // Faz 1 — CLI faces.
    pub const PING: &str = "ping";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
    pub const RUN: &str = "run";
    pub const LOG: &str = "log";
    pub const DIFF: &str = "diff";
    pub const MERGE: &str = "merge";
    pub const CLEANUP: &str = "cleanup";

    // Faz 2 — MCP tool faces (called by the divan-mcp server).
    pub const MCP_SEND_MESSAGE: &str = "mcp.send_message";
    pub const MCP_DELEGATE_TASK: &str = "mcp.delegate_task";
    pub const MCP_CLAIM_TASK: &str = "mcp.claim_task";
    pub const MCP_COMPLETE_TASK: &str = "mcp.complete_task";
    pub const MCP_PUBLISH_ARTIFACT: &str = "mcp.publish_artifact";
    pub const MCP_GET_ARTIFACT: &str = "mcp.get_artifact";
    pub const MCP_SUBSCRIBE: &str = "mcp.subscribe";
    pub const MCP_LIST_AGENTS: &str = "mcp.list_agents";

    // Faz 2 — hook faces (called by hook scripts).
    pub const HOOK_ACTIVITY: &str = "hook.activity";
    pub const HOOK_TURN_END: &str = "hook.turn_end";
    pub const HOOK_SESSION_IDLE: &str = "hook.session_idle";
    pub const HOOK_CONFIRM: &str = "hook.confirm";
}

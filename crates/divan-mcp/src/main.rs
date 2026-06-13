//! `divan-mcp` — the Divan MCP server face (impl plan §F2.3, spec §3.6-B).
//!
//! A stdio MCP server (rmcp 1.7) exposing the 8 Divan tools. Each tool is a thin
//! forwarder: it builds a JSON-RPC params object and calls the running Divan
//! daemon over its unix socket (reusing `divan_daemon::rpc`), then returns the
//! daemon's JSON result. Core orchestration logic lives in the daemon, not here.
//!
//! Tool/field descriptions are deliberately terse: these schemas sit in every
//! agent's context each turn (K2/K4 token budget). Schemas are flat, single-level.
//!
//! The proven recipe and rmcp 1.7.0 API gotchas come from spike S4
//! (`docs/spikes/s4_rmcp_toy_server.md`).

use std::path::PathBuf;

use divan_daemon::config::DaemonConfig;
use divan_daemon::protocol::{method, RpcRequest};
use divan_daemon::rpc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, ErrorData, Implementation, ServerCapabilities, ServerInfo,
};
// (Implementation::new used in get_info; from_build_env intentionally avoided.)
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Divan message rule: a message summary must be at most this many chars.
/// Enforced at the server edge (CLAUDE.md "Message Rules"; S4 §3.4).
const MAX_SUMMARY: usize = 400;

/// The MCP server face. Holds the macro-generated tool router plus the resolved
/// daemon socket path and this agent's identity (read once from the env).
struct DivanMcp {
    // The `#[tool_handler]` macro reads this field; the compiler can't see that.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    /// Path to the running daemon's unix socket.
    socket: PathBuf,
    /// This agent's id (the daemon-registered MCP config sets `DIVAN_AGENT_ID`).
    agent_id: String,
    /// Optional task/trace correlation, propagated onto `send_message` if set.
    task_id: Option<String>,
    trace_id: Option<String>,
}

impl DivanMcp {
    fn new() -> Self {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
        let socket = DaemonConfig::default_for_home(&home).socket_path;
        let agent_id = std::env::var("DIVAN_AGENT_ID").unwrap_or_else(|_| "mcp-agent".to_string());
        let task_id = std::env::var("DIVAN_TASK_ID")
            .ok()
            .filter(|s| !s.is_empty());
        let trace_id = std::env::var("DIVAN_TRACE_ID")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            tool_router: Self::tool_router(),
            socket,
            agent_id,
            task_id,
            trace_id,
        }
    }

    /// Forward one call to the daemon and map its `RpcResponse` to an MCP result.
    /// Daemon-down -> MCP internal error; daemon `ok:false` -> MCP internal error
    /// carrying the daemon's message; otherwise the result JSON as a text block.
    async fn forward(&self, method: &str, params: Value) -> Result<CallToolResult, ErrorData> {
        let resp = rpc::send(&self.socket, &RpcRequest::new(method, params))
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("divan daemon not running ({e})"), None)
            })?;
        if resp.ok {
            Ok(CallToolResult::success(vec![Content::text(
                resp.result.to_string(),
            )]))
        } else {
            Err(ErrorData::internal_error(
                resp.error.unwrap_or_else(|| "daemon error".to_string()),
                None,
            ))
        }
    }
}

// ---- Tool argument schemas (flat, single-level; terse descriptions, K2/K4). ----

#[derive(Debug, Deserialize, JsonSchema)]
struct SendMessageArgs {
    /// Recipient agent id; omit to broadcast.
    to: Option<String>,
    /// One of: handoff, review_done, question, status, alert.
    kind: String,
    /// Short summary, <= 400 chars (Divan message rule).
    summary: String,
    /// Optional artifact pointer (blake3 ref) for large content.
    artifact_ref: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DelegateTaskArgs {
    /// Task kind (e.g. write, review).
    kind: String,
    /// Artifact ref holding the task spec.
    spec_artifact: String,
    /// Optional short task title.
    title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClaimTaskArgs {
    /// Optional task kind filter; omit to claim the oldest open task.
    kind: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteTaskArgs {
    /// Id of the task to complete.
    task_id: String,
    /// Optional artifact ref holding the result.
    result_artifact: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PublishArtifactArgs {
    /// Raw content to store; server returns its blake3 ref.
    content: String,
    /// Optional short summary of the content.
    summary: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetArtifactArgs {
    /// Blake3 ref of the artifact to fetch.
    artifact_ref: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SubscribeArgs {
    /// Event kind to subscribe to (e.g. message, task).
    event_kind: String,
    /// Optional event filter object.
    filter: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListAgentsArgs {
    /// Optional capability filter.
    capability: Option<String>,
}

// ---- Tools: the 8-tool MCP face (spec §3.6-B). Each forwards to the daemon. ----

#[tool_router]
impl DivanMcp {
    #[tool(description = "Queue a message to another agent. summary must be <= 400 chars.")]
    async fn send_message(
        &self,
        Parameters(args): Parameters<SendMessageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        // Enforce the message rule at the server edge (S4 §3.4).
        if args.summary.chars().count() > MAX_SUMMARY {
            return Err(ErrorData::invalid_params(
                format!(
                    "summary too long: {} chars (max {MAX_SUMMARY})",
                    args.summary.chars().count()
                ),
                None,
            ));
        }
        let params = json!({
            "from": self.agent_id,
            "to": args.to,
            "kind": args.kind,
            "summary": args.summary,
            "artifact_ref": args.artifact_ref,
            "task_id": self.task_id,
            "trace_id": self.trace_id,
        });
        self.forward(method::MCP_SEND_MESSAGE, params).await
    }

    #[tool(description = "Create an open task another agent can claim.")]
    async fn delegate_task(
        &self,
        Parameters(args): Parameters<DelegateTaskArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({
            "agent_id": self.agent_id,
            "kind": args.kind,
            "spec_artifact": args.spec_artifact,
            "title": args.title,
        });
        self.forward(method::MCP_DELEGATE_TASK, params).await
    }

    #[tool(description = "Claim the oldest open task, optionally of a given kind.")]
    async fn claim_task(
        &self,
        Parameters(args): Parameters<ClaimTaskArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({
            "agent_id": self.agent_id,
            "kind": args.kind,
        });
        self.forward(method::MCP_CLAIM_TASK, params).await
    }

    #[tool(description = "Mark a task done and record its result artifact.")]
    async fn complete_task(
        &self,
        Parameters(args): Parameters<CompleteTaskArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({
            "task_id": args.task_id,
            "result_artifact": args.result_artifact,
        });
        self.forward(method::MCP_COMPLETE_TASK, params).await
    }

    #[tool(description = "Store content and return its blake3 ref.")]
    async fn publish_artifact(
        &self,
        Parameters(args): Parameters<PublishArtifactArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({
            "content": args.content,
            "summary": args.summary,
            "created_by": self.agent_id,
        });
        self.forward(method::MCP_PUBLISH_ARTIFACT, params).await
    }

    #[tool(description = "Fetch artifact content by its blake3 ref.")]
    async fn get_artifact(
        &self,
        Parameters(args): Parameters<GetArtifactArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({ "ref": args.artifact_ref });
        self.forward(method::MCP_GET_ARTIFACT, params).await
    }

    #[tool(description = "Subscribe to an event kind, with an optional filter.")]
    async fn subscribe(
        &self,
        Parameters(args): Parameters<SubscribeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({
            "agent_id": self.agent_id,
            "event_kind": args.event_kind,
            "filter": args.filter,
        });
        self.forward(method::MCP_SUBSCRIBE, params).await
    }

    #[tool(description = "List agents, optionally filtered by capability.")]
    async fn list_agents(
        &self,
        Parameters(args): Parameters<ListAgentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let params = json!({ "capability": args.capability });
        self.forward(method::MCP_LIST_AGENTS, params).await
    }
}

// `get_info` is synchronous; `#[tool_handler]` binds list_tools/call_tool (S4).
#[tool_handler]
impl ServerHandler for DivanMcp {
    fn get_info(&self) -> ServerInfo {
        let caps = ServerCapabilities::builder().enable_tools().build();
        // `from_build_env()` would capture rmcp's own crate env, not ours; use
        // `Implementation::new` with this crate's env! macros for correct name/version.
        ServerInfo::new(caps)
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Divan MCP face: send_message, delegate_task, claim_task, complete_task, \
                 publish_artifact, get_artifact, subscribe, list_agents. Replies to injected \
                 [DIVAN MESSAGES] blocks go through send_message, not free text.",
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs go to stderr; stdout is the MCP wire (stdio transport).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let server = DivanMcp::new();
    tracing::info!(
        agent_id = %server.agent_id,
        socket = %server.socket.display(),
        "divan-mcp serving 8 tools over stdio"
    );
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

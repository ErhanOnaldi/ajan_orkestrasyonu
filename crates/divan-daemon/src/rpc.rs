//! Unix-socket JSON-RPC server + client (impl plan §F1.3).
//!
//! One newline-delimited JSON request per connection; one JSON response. The
//! daemon stays single-process behind a lock file (spec §3.7); the CLI is a
//! thin client over this socket.

use crate::bus::{MessageBus, SendRequest};
use crate::protocol::{method, RpcRequest, RpcResponse, RunParams};
use crate::scheduler::Scheduler;
use divan_core::{AgentId, EventFilter, MessageKind, Subscription, TaskId, TraceId};
use divan_db::{AgentStore, SubscriptionStore, TaskStore, TraceStore};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// Shared daemon state handed to each connection (impl plan §F1.3/§F2.x).
#[derive(Clone)]
pub struct DaemonState {
    pub scheduler: Arc<Scheduler>,
    pub bus: MessageBus,
    pub stop: Arc<Notify>,
}

/// Serve the daemon RPC until a `shutdown` request (or `stop` notify) arrives.
pub async fn serve(
    socket_path: &Path,
    scheduler: Arc<Scheduler>,
    bus: MessageBus,
    stop: Arc<Notify>,
) -> anyhow::Result<()> {
    let state = DaemonState {
        scheduler,
        bus,
        stop: stop.clone(),
    };
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Stale socket cleanup (impl plan §F1.3).
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    tracing::info!(socket = %socket_path.display(), "divan daemon listening");

    loop {
        tokio::select! {
            _ = stop.notified() => {
                tracing::info!("divan daemon shutting down");
                break;
            }
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, state).await {
                        tracing::warn!(error = %e, "connection error");
                    }
                });
            }
        }
    }
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

async fn handle_conn(stream: UnixStream, state: DaemonState) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let resp = match serde_json::from_str::<RpcRequest>(line.trim()) {
        Ok(req) => dispatch(req, &state).await,
        Err(e) => RpcResponse::err(format!("bad request: {e}")),
    };
    let mut out = serde_json::to_string(&resp)?;
    out.push('\n');
    write_half.write_all(out.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

async fn dispatch(req: RpcRequest, state: &DaemonState) -> RpcResponse {
    let scheduler = &state.scheduler;
    match req.method.as_str() {
        method::PING => RpcResponse::ok(serde_json::json!({
            "pong": true, "pid": std::process::id(),
        })),
        method::STATUS => status(scheduler),
        method::SHUTDOWN => {
            state.stop.notify_one();
            RpcResponse::ok(serde_json::json!({"stopping": true}))
        }
        method::RUN => run(req, scheduler).await,
        method::LOG => log(req, scheduler),
        method::DIFF => match scheduler.diff_task(param_str(&req, "task_id")).await {
            Ok(diff) => RpcResponse::ok(serde_json::json!({"diff": diff})),
            Err(e) => RpcResponse::err(e.to_string()),
        },
        method::MERGE => match scheduler.merge_task(param_str(&req, "task_id")).await {
            Ok(()) => RpcResponse::ok(serde_json::json!({"merged": true})),
            Err(e) => RpcResponse::err(e.to_string()),
        },
        method::CLEANUP => match scheduler.cleanup_task(param_str(&req, "task_id")).await {
            Ok(()) => RpcResponse::ok(serde_json::json!({"cleaned": true})),
            Err(e) => RpcResponse::err(e.to_string()),
        },
        // ---- Phase 2: MCP tool faces (called by the divan-mcp server) ----
        method::MCP_SEND_MESSAGE => mcp_send_message(&req, state),
        method::MCP_SUBSCRIBE => mcp_subscribe(&req, state),
        method::MCP_LIST_AGENTS => mcp_list_agents(&req, state),
        method::MCP_PUBLISH_ARTIFACT => mcp_publish_artifact(&req, state),
        method::MCP_GET_ARTIFACT => mcp_get_artifact(&req, state),
        method::MCP_DELEGATE_TASK => mcp_delegate_task(&req, state),
        method::MCP_CLAIM_TASK => mcp_claim_task(&req, state),
        method::MCP_COMPLETE_TASK => mcp_complete_task(&req, state),
        // ---- Phase 2: hook faces (called by hook scripts) ----
        method::HOOK_ACTIVITY => hook_activity(&req, state),
        method::HOOK_TURN_END | method::HOOK_SESSION_IDLE => hook_pending_batch(&req, state),
        method::HOOK_CONFIRM => hook_confirm(&req, state),
        other => RpcResponse::err(format!("unknown method: {other}")),
    }
}

// ---- MCP tool handlers ----

fn mcp_send_message(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let p = &req.params;
    let from = match p.get("from").and_then(|v| v.as_str()) {
        Some(s) => AgentId::new(s),
        None => return RpcResponse::err("send_message: `from` required"),
    };
    let kind: MessageKind = match p
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s.into())).ok())
    {
        Some(k) => k,
        None => return RpcResponse::err("send_message: invalid `kind`"),
    };
    let send = SendRequest {
        from,
        to: p.get("to").and_then(|v| v.as_str()).map(AgentId::new),
        kind,
        summary: p
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        payload: p.get("payload").cloned(),
        artifact_ref: p
            .get("artifact_ref")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        task_id: p.get("task_id").and_then(|v| v.as_str()).map(TaskId::new),
        trace_id: p.get("trace_id").and_then(|v| v.as_str()).map(TraceId::new),
    };
    match state.bus.send(send) {
        Ok(crate::bus::SendOutcome::Direct(id)) => {
            RpcResponse::ok(serde_json::json!({"mode": "direct", "message_id": id.as_str()}))
        }
        Ok(crate::bus::SendOutcome::FanOut { source, copies }) => {
            RpcResponse::ok(serde_json::json!({
                "mode": "broadcast",
                "source": source.as_str(),
                "delivered": copies.len(),
            }))
        }
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn mcp_subscribe(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let p = &req.params;
    let agent_id = match p.get("agent_id").and_then(|v| v.as_str()) {
        Some(s) => AgentId::new(s),
        None => return RpcResponse::err("subscribe: `agent_id` required"),
    };
    let event_kind = p
        .get("event_kind")
        .and_then(|v| v.as_str())
        .unwrap_or(divan_core::EVENT_KIND_ANY)
        .to_string();
    let filter: Option<EventFilter> = p
        .get("filter")
        .and_then(|f| serde_json::from_value(f.clone()).ok());
    let sub = Subscription {
        agent_id,
        event_kind,
        filter,
    };
    match state.scheduler.db().subscribe(&sub) {
        Ok(()) => RpcResponse::ok(serde_json::json!({"subscribed": true})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn mcp_list_agents(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let cap_filter = req
        .params
        .get("capability")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            serde_json::from_value::<divan_core::Capability>(serde_json::Value::String(s.into()))
                .ok()
        });
    let agents = match state.scheduler.db().list_agents() {
        Ok(a) => a,
        Err(e) => return RpcResponse::err(e.to_string()),
    };
    let rows: Vec<_> = agents
        .iter()
        .filter(|a| cap_filter.map(|c| a.has(c)).unwrap_or(true))
        .map(|a| {
            serde_json::json!({
                "id": a.id.as_str(), "tool": a.tool.as_str(),
                "capabilities": a.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "skills": a.skills, "multi_turn": a.multi_turn,
            })
        })
        .collect();
    RpcResponse::ok(serde_json::json!({"agents": rows}))
}

fn mcp_publish_artifact(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let p = &req.params;
    let content = match (
        p.get("content").and_then(|v| v.as_str()),
        p.get("path").and_then(|v| v.as_str()),
    ) {
        (Some(c), _) => c.as_bytes().to_vec(),
        (None, Some(path)) => match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return RpcResponse::err(format!("publish_artifact read {path}: {e}")),
        },
        (None, None) => return RpcResponse::err("publish_artifact: `content` or `path` required"),
    };
    let summary = p.get("summary").and_then(|v| v.as_str());
    let mime = p.get("mime").and_then(|v| v.as_str());
    let by = p
        .get("created_by")
        .and_then(|v| v.as_str())
        .map(AgentId::new);
    match state.scheduler.artifacts().put(
        &content,
        mime,
        summary,
        by.as_ref(),
        divan_core::now_ms(),
    ) {
        Ok(r) => RpcResponse::ok(serde_json::json!({"ref": r.as_str()})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn mcp_get_artifact(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let r = divan_core::ArtifactRef::new(param_str(req, "ref"));
    match state.scheduler.artifacts().get(&r) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => RpcResponse::ok(serde_json::json!({"content": text})),
            Err(_) => RpcResponse::err("artifact is not valid UTF-8 (binary get is Faz 2+)"),
        },
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn parse_kind(s: &str) -> Option<divan_core::TaskKind> {
    serde_json::from_value(serde_json::Value::String(s.into())).ok()
}

/// `delegate_task`: create an open task another agent can claim (swarm-style,
/// spec §3.6-B). Heavy spec content stays in the artifact (`spec_artifact`, K4).
fn mcp_delegate_task(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let p = &req.params;
    let kind = match p.get("kind").and_then(|v| v.as_str()).and_then(parse_kind) {
        Some(k) => k,
        None => return RpcResponse::err("delegate_task: invalid `kind`"),
    };
    let title = p
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("delegated task");
    let now = divan_core::now_ms();
    let task = divan_core::Task {
        id: TaskId::new(format!("deleg-{now}")),
        parent_id: p.get("parent_id").and_then(|v| v.as_str()).map(TaskId::new),
        kind,
        title: title.to_string(),
        spec_ref: p
            .get("spec_artifact")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        state: divan_core::TaskState::Open,
        assignee: None,
        worktree: None,
        max_runtime_secs: None,
        trace_id: p
            .get("trace_id")
            .and_then(|v| v.as_str())
            .map(TraceId::new)
            .unwrap_or_else(|| TraceId::new(format!("trace-{now}"))),
        created_at: now,
        updated_at: now,
    };
    match state.scheduler.db().create_task(&task) {
        Ok(()) => RpcResponse::ok(serde_json::json!({"task_id": task.id.as_str()})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

/// `claim_task`: claim the oldest open task (optionally of a given kind) for an
/// agent; transitions open -> claimed (deterministic, K2).
fn mcp_claim_task(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let agent = match req.params.get("agent_id").and_then(|v| v.as_str()) {
        Some(s) => AgentId::new(s),
        None => return RpcResponse::err("claim_task: `agent_id` required"),
    };
    let kind_filter = req
        .params
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(parse_kind);
    let db = state.scheduler.db();
    let tasks = match db.list_tasks() {
        Ok(t) => t,
        Err(e) => return RpcResponse::err(e.to_string()),
    };
    let candidate = tasks.into_iter().find(|t| {
        t.state == divan_core::TaskState::Open && kind_filter.map(|k| t.kind == k).unwrap_or(true)
    });
    let Some(task) = candidate else {
        return RpcResponse::ok(serde_json::json!({"claimed": serde_json::Value::Null}));
    };
    if let Err(e) = db.assign(&task.id, &agent, None) {
        return RpcResponse::err(e.to_string());
    }
    match db.transition(
        &task.id,
        divan_core::TaskState::Claimed,
        divan_core::TransitionReason::Other("claimed via MCP".into()),
    ) {
        Ok(()) => RpcResponse::ok(serde_json::json!({"claimed": task.id.as_str()})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

/// `complete_task`: drive a claimed/working task to `done` and record the
/// result artifact ref (spec §3.6-B). Unblocked dependents become claimable.
fn mcp_complete_task(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let id = TaskId::new(param_str(req, "task_id"));
    let db = state.scheduler.db();
    let task = match db.get_task(&id) {
        Ok(Some(t)) => t,
        Ok(None) => return RpcResponse::err(format!("complete_task: unknown task {id}")),
        Err(e) => return RpcResponse::err(e.to_string()),
    };
    // Walk to done through legal transitions (claimed -> working -> done).
    let path = match task.state {
        divan_core::TaskState::Claimed => {
            vec![divan_core::TaskState::Working, divan_core::TaskState::Done]
        }
        divan_core::TaskState::Working | divan_core::TaskState::Review => {
            vec![divan_core::TaskState::Done]
        }
        divan_core::TaskState::Open => {
            vec![
                divan_core::TaskState::Claimed,
                divan_core::TaskState::Working,
                divan_core::TaskState::Done,
            ]
        }
        s => {
            return RpcResponse::err(format!(
                "complete_task: task is {} (not completable)",
                s.as_str()
            ))
        }
    };
    for st in path {
        if let Err(e) = db.transition(&id, st, divan_core::TransitionReason::Completed) {
            return RpcResponse::err(e.to_string());
        }
    }
    RpcResponse::ok(serde_json::json!({
        "task_id": id.as_str(),
        "result_artifact": req.params.get("result_artifact"),
    }))
}

// ---- hook handlers ----

fn hook_activity(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    // Record an activity heartbeat: mark the agent busy + bind its session.
    let agent = AgentId::new(param_str(req, "agent_id"));
    let session = req.params.get("session_id").and_then(|v| v.as_str());
    let _ = state
        .scheduler
        .db()
        .set_agent_status(&agent, divan_core::AgentStatus::Busy, session);
    RpcResponse::ok(serde_json::json!({"ack": true}))
}

/// Turn-boundary / idle wake: return the pending injection block (K5). Does NOT
/// mark delivered — the hook confirms after injecting (so a failed injection
/// leaves messages pending, impl plan §F2.5).
fn hook_pending_batch(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let agent = AgentId::new(param_str(req, "agent_id"));
    match state.bus.pending_batch(&agent) {
        Ok(Some(batch)) => RpcResponse::ok(serde_json::json!({
            "has_messages": true,
            "text": batch.text,
            "message_ids": batch.message_ids.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        })),
        Ok(None) => RpcResponse::ok(serde_json::json!({"has_messages": false})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn hook_confirm(req: &RpcRequest, state: &DaemonState) -> RpcResponse {
    let agent = AgentId::new(param_str(req, "agent_id"));
    let ids: Vec<divan_core::MessageId> = req
        .params
        .get("message_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(divan_core::MessageId::new)
                .collect()
        })
        .unwrap_or_default();
    match state.bus.confirm_delivery(&ids, &agent) {
        Ok(n) => RpcResponse::ok(serde_json::json!({"delivered": n})),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn param_str<'a>(req: &'a RpcRequest, key: &str) -> &'a str {
    req.params.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn status(scheduler: &Scheduler) -> RpcResponse {
    let db = scheduler.db();
    let agents = match db.list_agents() {
        Ok(a) => a,
        Err(e) => return RpcResponse::err(e.to_string()),
    };
    let tasks = match db.list_tasks() {
        Ok(t) => t,
        Err(e) => return RpcResponse::err(e.to_string()),
    };
    let agents_json: Vec<_> = agents
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id.as_str(), "tool": a.tool.as_str(),
                "status": format!("{:?}", a.status).to_lowercase(),
                "cost_class": a.cost_class,
            })
        })
        .collect();
    let tasks_json: Vec<_> = tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id.as_str(), "kind": t.kind.as_str(), "state": t.state.as_str(),
                "assignee": t.assignee.as_ref().map(|a| a.as_str()),
            })
        })
        .collect();
    RpcResponse::ok(serde_json::json!({"agents": agents_json, "tasks": tasks_json}))
}

async fn run(req: RpcRequest, scheduler: &Scheduler) -> RpcResponse {
    let params: RunParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => return RpcResponse::err(format!("bad run params: {e}")),
    };
    let repo = Path::new(&params.repo);
    if !repo.join(".git").exists() {
        return RpcResponse::err(format!("{} is not a git repository", params.repo));
    }
    match scheduler
        .run_write_review(
            &params.title,
            &params.spec,
            repo,
            &AgentId::new(params.writer),
            &AgentId::new(params.reviewer),
        )
        .await
    {
        Ok(report) => RpcResponse::ok(serde_json::to_value(report).unwrap_or_default()),
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

fn log(req: RpcRequest, scheduler: &Scheduler) -> RpcResponse {
    let db = scheduler.db();
    // Resolve a trace: explicit --trace, else --task -> its trace, else recent.
    let events = if let Some(trace) = req.params.get("trace").and_then(|v| v.as_str()) {
        db.timeline(&TraceId::new(trace))
    } else if let Some(task) = req.params.get("task").and_then(|v| v.as_str()) {
        match db.get_task(&TaskId::new(task)) {
            Ok(Some(t)) => db.timeline(&t.trace_id),
            Ok(None) => return RpcResponse::err(format!("unknown task {task}")),
            Err(e) => return RpcResponse::err(e.to_string()),
        }
    } else {
        db.recent_events(200)
    };
    match events {
        Ok(evs) => {
            let rows: Vec<_> = evs
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "ts": e.ts, "event": e.event.as_str(),
                        "agent": e.agent_id.as_ref().map(|a| a.as_str()),
                        "trace": e.trace_id.as_str(), "data": e.data,
                    })
                })
                .collect();
            RpcResponse::ok(serde_json::json!({"events": rows}))
        }
        Err(e) => RpcResponse::err(e.to_string()),
    }
}

/// Client: send one request to the daemon socket and read the response.
pub async fn send(socket_path: &Path, req: &RpcRequest) -> anyhow::Result<RpcResponse> {
    let stream = UnixStream::connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    let mut payload = serde_json::to_string(req)?;
    payload.push('\n');
    write_half.write_all(payload.as_bytes()).await?;
    write_half.flush().await?;
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(serde_json::from_str(line.trim())?)
}

/// Is a daemon already responding on this socket? (used by `divan up`).
pub async fn is_alive(socket_path: &Path) -> bool {
    if !socket_path.exists() {
        return false;
    }
    matches!(
        send(socket_path, &RpcRequest::new(method::PING, serde_json::Value::Null)).await,
        Ok(r) if r.ok
    )
}

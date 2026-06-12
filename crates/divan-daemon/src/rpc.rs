//! Unix-socket JSON-RPC server + client (impl plan §F1.3).
//!
//! One newline-delimited JSON request per connection; one JSON response. The
//! daemon stays single-process behind a lock file (spec §3.7); the CLI is a
//! thin client over this socket.

use crate::protocol::{method, RpcRequest, RpcResponse, RunParams};
use crate::scheduler::Scheduler;
use divan_core::{AgentId, TaskId, TraceId};
use divan_db::{AgentStore, TaskStore, TraceStore};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// Serve the daemon RPC until a `shutdown` request (or `stop` notify) arrives.
pub async fn serve(
    socket_path: &Path,
    scheduler: Arc<Scheduler>,
    stop: Arc<Notify>,
) -> anyhow::Result<()> {
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
                let sched = scheduler.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, sched, stop).await {
                        tracing::warn!(error = %e, "connection error");
                    }
                });
            }
        }
    }
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

async fn handle_conn(
    stream: UnixStream,
    scheduler: Arc<Scheduler>,
    stop: Arc<Notify>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    let resp = match serde_json::from_str::<RpcRequest>(line.trim()) {
        Ok(req) => dispatch(req, scheduler, stop).await,
        Err(e) => RpcResponse::err(format!("bad request: {e}")),
    };
    let mut out = serde_json::to_string(&resp)?;
    out.push('\n');
    write_half.write_all(out.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

async fn dispatch(req: RpcRequest, scheduler: Arc<Scheduler>, stop: Arc<Notify>) -> RpcResponse {
    match req.method.as_str() {
        method::PING => RpcResponse::ok(serde_json::json!({
            "pong": true, "pid": std::process::id(),
        })),
        method::STATUS => status(&scheduler),
        method::SHUTDOWN => {
            stop.notify_one();
            RpcResponse::ok(serde_json::json!({"stopping": true}))
        }
        method::RUN => run(req, &scheduler).await,
        method::LOG => log(req, &scheduler),
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
        other => RpcResponse::err(format!("unknown method: {other}")),
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

//! Socket-level "toy MCP client" tests for the Faz 2 messaging RPCs
//! (impl plan §F2.3/§F2.5 acceptance): send a message, peek the turn-boundary
//! batch via the hook face, confirm delivery, and verify the queue drains.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use divan_daemon::bus::MessageBus;
use divan_daemon::config::DaemonConfig;
use divan_daemon::protocol::{method, RpcRequest};
use divan_daemon::scheduler::Scheduler;
use divan_daemon::{rpc, worktree::WorktreeManager};
use divan_db::{AgentStore, ArtifactStore, Db};
use serde_json::{json, Value};
use tokio::sync::Notify;

fn card(id: &str, caps: Vec<divan_core::Capability>) -> divan_core::AgentCard {
    divan_core::AgentCard {
        id: divan_core::AgentId::new(id),
        tool: divan_core::AgentTool::Claude,
        display_name: None,
        capabilities: caps,
        cost_class: 1,
        skills: vec![],
        delivery: vec![divan_core::DeliveryKind::Mcp],
        multi_turn: true,
        status: divan_core::AgentStatus::Idle,
        session_id: None,
        registered_at: 0,
    }
}

async fn spawn_daemon(
    dir: &Path,
) -> (
    std::path::PathBuf,
    Db,
    Arc<Notify>,
    tokio::task::JoinHandle<()>,
) {
    let sock = dir.join("d.sock");
    let db = Db::open_in_memory().unwrap();
    db.upsert_agent(&card("claude-1", vec![divan_core::Capability::Broadcast]))
        .unwrap();
    db.upsert_agent(&card("codex-1", vec![divan_core::Capability::Read]))
        .unwrap();
    let artifacts = Arc::new(ArtifactStore::new(db.clone(), dir.join("art")));
    let worktrees = Arc::new(WorktreeManager::new(dir.join("wt")));
    let cfg = DaemonConfig::default_for_home(Path::new("/tmp"));
    let scheduler = Arc::new(Scheduler::new(db.clone(), artifacts, worktrees, cfg));
    let bus = MessageBus::new(db.clone());
    let stop = Arc::new(Notify::new());
    let (sock2, stop2) = (sock.clone(), stop.clone());
    let handle = tokio::spawn(async move {
        let _ = rpc::serve(&sock2, scheduler, bus, stop2).await;
    });
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (sock, db, stop, handle)
}

async fn call(sock: &Path, m: &str, params: Value) -> divan_daemon::protocol::RpcResponse {
    rpc::send(sock, &RpcRequest::new(m, params)).await.unwrap()
}

#[tokio::test]
async fn send_then_hook_deliver_and_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, _db, _stop, handle) = spawn_daemon(dir.path()).await;

    // claude-1 sends a direct question to codex-1.
    let r = call(
        &sock,
        method::MCP_SEND_MESSAGE,
        json!({"from":"claude-1","to":"codex-1","kind":"question",
               "summary":"where is the spec?","task_id":"impl-1","trace_id":"tr-1"}),
    )
    .await;
    assert!(r.ok, "send failed: {:?}", r.error);
    assert_eq!(r.result["mode"], "direct");

    // codex-1's turn boundary: the hook peeks the injection batch.
    let r = call(&sock, method::HOOK_TURN_END, json!({"agent_id":"codex-1"})).await;
    assert_eq!(r.result["has_messages"], true);
    let text = r.result["text"].as_str().unwrap();
    assert!(text.contains("[DIVAN MESSAGES]"));
    assert!(text.contains("send_message MCP tool")); // the fixed instruction
    assert!(text.contains("kind=question"));
    let ids = r.result["message_ids"].clone();

    // Until confirmed, the message stays pending (peek does not deliver).
    let r2 = call(&sock, method::HOOK_TURN_END, json!({"agent_id":"codex-1"})).await;
    assert_eq!(r2.result["has_messages"], true, "peek must not deliver");

    // Hook confirms after injecting -> delivered, queue drains.
    let r = call(
        &sock,
        method::HOOK_CONFIRM,
        json!({"agent_id":"codex-1","message_ids":ids}),
    )
    .await;
    assert_eq!(r.result["delivered"], 1);
    let r = call(&sock, method::HOOK_TURN_END, json!({"agent_id":"codex-1"})).await;
    assert_eq!(r.result["has_messages"], false);

    let _ = call(&sock, method::SHUTDOWN, Value::Null).await;
    let _ = handle.await;
}

#[tokio::test]
async fn broadcast_requires_subscription_and_capability() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, _db, _stop, handle) = spawn_daemon(dir.path()).await;

    // codex-1 subscribes to questions.
    let r = call(
        &sock,
        method::MCP_SUBSCRIBE,
        json!({"agent_id":"codex-1","event_kind":"question"}),
    )
    .await;
    assert!(r.ok);

    // claude-1 (has broadcast) broadcasts a question -> reaches codex-1.
    let r = call(
        &sock,
        method::MCP_SEND_MESSAGE,
        json!({"from":"claude-1","kind":"question","summary":"anyone?","trace_id":"tr-1"}),
    )
    .await;
    assert!(r.ok, "broadcast failed: {:?}", r.error);
    assert_eq!(r.result["mode"], "broadcast");
    assert_eq!(r.result["delivered"], 1);

    // codex-1 (no broadcast capability) is denied.
    let r = call(
        &sock,
        method::MCP_SEND_MESSAGE,
        json!({"from":"codex-1","kind":"question","summary":"me too?","trace_id":"tr-1"}),
    )
    .await;
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("broadcast"));

    let _ = call(&sock, method::SHUTDOWN, Value::Null).await;
    let _ = handle.await;
}

#[tokio::test]
async fn publish_then_get_artifact_and_list_agents() {
    let dir = tempfile::tempdir().unwrap();
    let (sock, _db, _stop, handle) = spawn_daemon(dir.path()).await;

    let r = call(
        &sock,
        method::MCP_PUBLISH_ARTIFACT,
        json!({"content":"review: looks good","mime":"text/markdown","created_by":"codex-1"}),
    )
    .await;
    assert!(r.ok);
    let aref = r.result["ref"].as_str().unwrap().to_string();
    let r = call(&sock, method::MCP_GET_ARTIFACT, json!({"ref": aref})).await;
    assert_eq!(r.result["content"], "review: looks good");

    let r = call(
        &sock,
        method::MCP_LIST_AGENTS,
        json!({"capability":"broadcast"}),
    )
    .await;
    let agents = r.result["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "claude-1");

    let _ = call(&sock, method::SHUTDOWN, Value::Null).await;
    let _ = handle.await;
}

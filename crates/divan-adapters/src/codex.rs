//! Codex adapter (spec §3.3 TAM, spike S2).
//!
//! Spawn: `codex exec --json --skip-git-repo-check --sandbox workspace-write
//! -C <worktree> -c approval_policy=never <prompt> < /dev/null`; review tasks
//! use `--sandbox read-only` (spike S2 §6). Output is JSONL with a flat shape:
//! top-level `type`, `thread_id`, `item_type`, `command`, `exit_code`,
//! `status`, `changes`, `usage` (codex-cli 0.139.0, spike S2 §3).
//!
//! Security: the adapter NEVER passes `--dangerously-bypass-approvals-and-sandbox`
//! (spike S2 §2). Isolation = sandbox + worktree path.

use crate::{
    AdapterError, AdapterResult, AgentAdapter, DeliveryReceipt, EventStream, MessageBatch,
    SessionHandle, SpawnCtx,
};
use async_trait::async_trait;
use divan_core::Usage;
use divan_core::{
    AgentCard, AgentId, AgentStatus, AgentTool, Capability, DeliveryKind, FileChangeKind,
    NormalizedEvent,
};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

fn change_kind(kind: &str) -> FileChangeKind {
    match kind {
        "add" => FileChangeKind::Added,
        "delete" => FileChangeKind::Deleted,
        // "modify" and anything else default to Modified.
        _ => FileChangeKind::Modified,
    }
}

/// Extract the codex `thread_id` from a `thread.started` line (resume key).
pub fn extract_thread_id(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? == "thread.started" {
        return v
            .get("thread_id")
            .and_then(|t| t.as_str())
            .map(str::to_owned);
    }
    None
}

/// Map a single codex `exec --json` line to zero or more normalized events
/// (spike S2 §5). Unknown lines yield an empty vec; SessionEnd is emitted by the
/// daemon wrapper on process exit (codex has no explicit session-end line).
pub fn parse_line(line: &str) -> Vec<NormalizedEvent> {
    let line = line.trim();
    if line.is_empty() {
        return vec![];
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return vec![NormalizedEvent::fatal(format!("codex parse error: {e}"))],
    };

    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        // The real codex 0.139.0 nests item fields under `item` (verified live);
        // discriminate on `item.type`.
        "item.completed" => v.get("item").map(parse_item).unwrap_or_default(),
        "turn.completed" => vec![NormalizedEvent::TurnEnd],
        // thread.started handled via extract_thread_id; turn.started / item.started ignored.
        _ => vec![],
    }
}

/// Map a completed `item` object to normalized events. `item` is the nested
/// object from an `item.completed` line.
fn parse_item(item: &Value) -> Vec<NormalizedEvent> {
    match item.get("type").and_then(|t| t.as_str()) {
        Some("command_execution") => {
            let command = item
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            vec![NormalizedEvent::ToolCall {
                name: "shell".into(),
                input: Some(serde_json::json!({
                    "command": command,
                    "exit_code": item.get("exit_code").cloned().unwrap_or(Value::Null),
                    "status": item.get("status").cloned().unwrap_or(Value::Null),
                })),
            }]
        }
        Some("mcp_tool_call") => vec![NormalizedEvent::ToolCall {
            name: item
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("mcp_tool")
                .to_string(),
            input: item.get("changes").cloned(),
        }],
        Some("file_change") => item
            .get("changes")
            .and_then(|c| c.as_array())
            .map(|changes| {
                changes
                    .iter()
                    .filter_map(|ch| {
                        let path = ch.get("path")?.as_str()?.to_string();
                        let kind =
                            change_kind(ch.get("kind").and_then(|k| k.as_str()).unwrap_or(""));
                        Some(NormalizedEvent::FileEdit { path, kind })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        // agent_message is assistant text — captured separately as final_text.
        _ => vec![],
    }
}

/// A live child session owned by the adapter between `spawn` and `observe`.
struct ChildSession {
    child: Child,
    stdout: Option<ChildStdout>,
}

/// The Codex adapter. Like [`crate::claude::ClaudeAdapter`] it owns spawned
/// children; unlike claude, codex emits no session-end line, so `observe`
/// derives `SessionEnd{ok}` from the child's process exit status (spike S2 §5).
pub struct CodexAdapter {
    pub id: AgentId,
    sessions: Arc<Mutex<HashMap<String, ChildSession>>>,
}

impl CodexAdapter {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(id),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn card(&self) -> AgentCard {
        AgentCard {
            id: self.id.clone(),
            tool: AgentTool::Codex,
            display_name: Some("Codex".into()),
            capabilities: vec![Capability::Read], // reviewer default (impl plan §8.1)
            cost_class: 4,
            skills: vec!["review".into(), "test".into()],
            delivery: vec![DeliveryKind::Mcp, DeliveryKind::Resume],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: divan_core::now_ms(),
        }
    }

    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle> {
        let mut cmd = Command::new("codex");
        cmd.arg("exec").arg("--json").arg("--skip-git-repo-check");
        if ctx.read_only {
            cmd.arg("--sandbox").arg("read-only");
        } else {
            cmd.arg("--sandbox")
                .arg("workspace-write")
                .arg("-c")
                .arg("approval_policy=never");
        }
        if let Some(wt) = &ctx.worktree {
            cmd.arg("-C").arg(wt);
        }
        cmd.arg(&ctx.prompt);
        for (k, val) in &ctx.env {
            cmd.env(k, val);
        }
        cmd.stdin(Stdio::null()) // required, else exec blocks on stdin (spike S2 §2)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("codex spawn failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = ctx.span_id.clone();
        self.sessions
            .lock()
            .await
            .insert(key.clone(), ChildSession { child, stdout });
        Ok(SessionHandle {
            tool: AgentTool::Codex,
            pid,
            session_id: None,
            task_id: key,
            started_at: divan_core::now_ms(),
        })
    }

    async fn observe(&self, handle: &SessionHandle) -> AdapterResult<EventStream> {
        let stdout = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(&handle.task_id)
                .ok_or_else(|| AdapterError::fatal("codex observe: unknown session"))?;
            session.stdout.take()
        };
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sessions = self.sessions.clone();
        let key = handle.task_id.clone();
        tokio::spawn(async move {
            let mut tid = None;
            let mut final_text = None;
            if let Some(stdout) = stdout {
                stream_child_stdout(stdout, tx.clone(), &mut tid, &mut final_text).await;
            }
            // Codex emits no session-end line: derive ok from the process exit.
            // If `kill` already removed the child, treat it as a failed session.
            let child = sessions.lock().await.remove(&key).map(|s| s.child);
            let ok = match child {
                Some(mut c) => c.wait().await.map(|s| s.success()).unwrap_or(false),
                None => false,
            };
            let _ = tx
                .send(NormalizedEvent::SessionEnd {
                    ok,
                    usage: Usage::default(),
                    final_text,
                })
                .await;
        });
        Ok(rx)
    }

    async fn deliver(
        &self,
        _handle: &SessionHandle,
        _batch: MessageBatch,
    ) -> AdapterResult<DeliveryReceipt> {
        Err(AdapterError::fatal("codex deliver: MCP delivery is Faz 2"))
    }

    async fn resume(&self, session_id: &str, prompt: &str) -> AdapterResult<SessionHandle> {
        let mut cmd = Command::new("codex");
        cmd.arg("exec")
            .arg("resume")
            .arg(session_id)
            .arg("--json")
            .arg("--skip-git-repo-check")
            .arg(prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("codex resume failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = format!("resume:{session_id}");
        self.sessions
            .lock()
            .await
            .insert(key.clone(), ChildSession { child, stdout });
        Ok(SessionHandle {
            tool: AgentTool::Codex,
            pid,
            session_id: Some(session_id.to_string()),
            task_id: key,
            started_at: divan_core::now_ms(),
        })
    }

    async fn kill(&self, handle: &SessionHandle) -> AdapterResult<()> {
        if let Some(mut session) = self.sessions.lock().await.remove(&handle.task_id) {
            let _ = session.child.kill().await;
        } else if let Some(pid) = handle.pid {
            let _ = Command::new("kill").arg(pid.to_string()).status().await;
        }
        Ok(())
    }
}

/// Extract the text of an `agent_message` item line (the reviewer's prose).
/// The item is nested under `item` in real codex output (verified live).
fn extract_agent_message(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let item = v.get("item")?;
    if item.get("type").and_then(|t| t.as_str()) == Some("agent_message") {
        return item
            .get("text")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
    }
    None
}

/// Stream a codex child's stdout into normalized events, capturing `thread_id`
/// and the last `agent_message` text (the review/implement result, spike S2 §5).
pub async fn stream_child_stdout(
    stdout: impl tokio::io::AsyncRead + Unpin,
    tx: tokio::sync::mpsc::Sender<NormalizedEvent>,
    thread_id_out: &mut Option<String>,
    final_text_out: &mut Option<String>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if thread_id_out.is_none() {
            if let Some(tid) = extract_thread_id(&line) {
                *thread_id_out = Some(tid);
            }
        }
        if let Some(text) = extract_agent_message(&line) {
            *final_text_out = Some(text); // keep the latest agent message
        }
        for ev in parse_line(&line) {
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures with the REAL codex 0.139.0 shape: item fields nested under
    // `item` (verified live, 2026-06-12). The earlier spike capture was
    // jq-projected/flattened, which did not match the wire format.
    const STARTED: &str = r#"{"type":"thread.started","thread_id":"019eb6f5-ef28"}"#;
    const MSG: &str = r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"REVIEW OK"}}"#;
    const CMD: &str = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'ls'","exit_code":0,"status":"completed"}}"#;
    const FILE: &str = r#"{"type":"item.completed","item":{"id":"item_1","type":"file_change","status":"completed","changes":[{"path":"/tmp/divan_s2w/hello.txt","kind":"add"}]}}"#;
    const TURN: &str =
        r#"{"type":"turn.completed","usage":{"input_tokens":16948,"output_tokens":26}}"#;

    #[test]
    fn extracts_thread_id() {
        assert_eq!(extract_thread_id(STARTED).as_deref(), Some("019eb6f5-ef28"));
        assert_eq!(extract_thread_id(MSG), None);
    }

    #[test]
    fn agent_message_is_text_not_an_event_but_is_captured() {
        // Not a normalized event, but its text is captured as final_text.
        assert!(parse_line(MSG).is_empty());
        assert_eq!(extract_agent_message(MSG).as_deref(), Some("REVIEW OK"));
        assert_eq!(extract_agent_message(CMD), None);
    }

    #[test]
    fn command_execution_is_toolcall_shell() {
        let evs = parse_line(CMD);
        assert!(matches!(
            &evs[..],
            [NormalizedEvent::ToolCall { name, .. }] if name == "shell"
        ));
    }

    #[test]
    fn file_change_native_kind() {
        let evs = parse_line(FILE);
        assert_eq!(
            evs,
            vec![NormalizedEvent::FileEdit {
                path: "/tmp/divan_s2w/hello.txt".into(),
                kind: FileChangeKind::Added,
            }]
        );
    }

    #[test]
    fn turn_completed_is_turn_end() {
        assert_eq!(parse_line(TURN), vec![NormalizedEvent::TurnEnd]);
    }

    #[test]
    fn malformed_is_fatal() {
        assert!(matches!(
            parse_line("nope").as_slice(),
            [NormalizedEvent::Error { class, .. }] if *class == divan_core::AgentErrorClass::Fatal
        ));
    }

    /// Real-CLI contract test (impl plan §10.3): off by default, run with
    /// `DIVAN_TEST_CODEX=1 cargo test -p divan-adapters real_codex`.
    #[tokio::test]
    async fn real_codex_session_emits_session_end() {
        if std::env::var("DIVAN_TEST_CODEX").is_err() {
            eprintln!("skip: set DIVAN_TEST_CODEX=1 to run the real codex contract test");
            return;
        }
        use crate::SpawnCtx;
        let adapter = CodexAdapter::new("codex-1");
        let ctx = SpawnCtx {
            worktree: Some(std::env::temp_dir()),
            env: std::collections::HashMap::new(),
            trace_id: "t".into(),
            span_id: "real-codex".into(),
            artifact_refs: vec![],
            read_only: true,
            prompt: "Reply with exactly the text: HELLO_DIVAN".into(),
            max_turns: Some(1),
        };
        let handle = adapter.spawn(ctx).await.expect("spawn codex");
        let mut rx = adapter.observe(&handle).await.expect("observe codex");
        let mut ended_ok = None;
        let mut saw_text = false;
        while let Some(ev) = rx.recv().await {
            if let NormalizedEvent::SessionEnd { ok, final_text, .. } = ev {
                ended_ok = Some(ok);
                saw_text = final_text.is_some();
            }
        }
        assert_eq!(ended_ok, Some(true), "codex session ended successfully");
        assert!(saw_text, "codex final agent message captured");
    }
}

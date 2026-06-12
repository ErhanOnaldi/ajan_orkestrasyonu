//! Claude Code adapter — reference adapter (spec §3.3 TAM, spike S1).
//!
//! Spawn: `claude -p <prompt> --output-format stream-json --verbose
//! --max-turns N [--permission-mode acceptEdits]`, cwd = worktree,
//! stdin = /dev/null (spike S1 §6). Output is JSONL; [`parse_line`] maps each
//! line to zero or more [`NormalizedEvent`]s.

use crate::{
    AdapterError, AdapterResult, AgentAdapter, DeliveryReceipt, EventStream, MessageBatch,
    SessionHandle, SpawnCtx,
};
use async_trait::async_trait;
use divan_core::{
    AgentCard, AgentId, AgentStatus, AgentTool, Capability, DeliveryKind, FileChangeKind,
    NormalizedEvent, Usage,
};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

/// Tool names that imply a file edit (spike S1 §5). `MultiEdit`/`NotebookEdit`
/// also touch files; kind is heuristic for claude (Write => Added else Modified).
fn file_edit_kind(tool_name: &str) -> Option<FileChangeKind> {
    match tool_name {
        "Write" => Some(FileChangeKind::Added),
        "Edit" | "MultiEdit" | "NotebookEdit" => Some(FileChangeKind::Modified),
        _ => None,
    }
}

/// Extract the claude `session_id` from a `system/init` line (resume key).
pub fn extract_session_id(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? == "system" {
        return v
            .get("session_id")
            .and_then(|s| s.as_str())
            .map(str::to_owned);
    }
    None
}

/// Map a single claude `stream-json` line to zero or more normalized events
/// (spike S1 §5). Unknown lines yield an empty vec — the stream is never broken.
pub fn parse_line(line: &str) -> Vec<NormalizedEvent> {
    let line = line.trim();
    if line.is_empty() {
        return vec![];
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // A malformed line is a parse failure for that line only (spike S1 §6).
        Err(e) => return vec![NormalizedEvent::fatal(format!("claude parse error: {e}"))],
    };

    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "assistant" => parse_assistant(&v),
        "system" => match v.get("subtype").and_then(|s| s.as_str()) {
            Some("post_turn_summary") => vec![NormalizedEvent::TurnEnd],
            _ => vec![], // init etc. handled via extract_session_id
        },
        "result" => {
            let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
            // `result.result` is the agent's final text (spike S1 §3).
            let final_text = v
                .get("result")
                .and_then(|r| r.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            vec![NormalizedEvent::SessionEnd {
                ok: !is_error,
                usage: parse_usage(v.get("usage")),
                final_text,
            }]
        }
        // rate_limit_event / user(tool_result) are telemetry, not normalized events.
        _ => vec![],
    }
}

fn parse_assistant(v: &Value) -> Vec<NormalizedEvent> {
    let mut out = vec![];
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());
    let Some(blocks) = content else {
        return out;
    };
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            let name = block
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let input = block.get("input").cloned();
            if let (Some(kind), Some(path)) = (
                file_edit_kind(&name),
                block
                    .get("input")
                    .and_then(|i| i.get("file_path"))
                    .and_then(|p| p.as_str()),
            ) {
                out.push(NormalizedEvent::FileEdit {
                    path: path.to_string(),
                    kind,
                });
            }
            out.push(NormalizedEvent::ToolCall { name, input });
        }
    }
    out
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let Some(u) = usage else {
        return Usage::default();
    };
    let get = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    Usage {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_read_tokens: get("cache_read_input_tokens"),
    }
}

/// A live child session owned by the adapter between `spawn` and `observe`.
struct ChildSession {
    child: Child,
    stdout: Option<ChildStdout>,
}

/// The Claude Code adapter. Owns spawned children in an internal registry keyed
/// by the session's task/span id so `observe` can stream the same process's
/// stdout and `kill` can terminate it (the daemon never sees the raw `Child`).
pub struct ClaudeAdapter {
    pub id: AgentId,
    sessions: Arc<Mutex<HashMap<String, ChildSession>>>,
}

impl ClaudeAdapter {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(id),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl AgentAdapter for ClaudeAdapter {
    fn card(&self) -> AgentCard {
        AgentCard {
            id: self.id.clone(),
            tool: AgentTool::Claude,
            display_name: Some("Claude Code".into()),
            capabilities: vec![Capability::Read, Capability::Write, Capability::Delegate],
            cost_class: 5,
            skills: vec!["implement".into(), "architecture".into()],
            // hook > MCP > resume (spec §3.6); reference adapter supports all.
            delivery: vec![DeliveryKind::Hook, DeliveryKind::Mcp, DeliveryKind::Resume],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: divan_core::now_ms(),
        }
    }

    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle> {
        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .arg(&ctx.prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
        if let Some(n) = ctx.max_turns {
            cmd.arg("--max-turns").arg(n.to_string());
        }
        // Read-only review tasks must not write; otherwise accept edits in the
        // worktree (worktree boundary is enforced by the Policy Engine, Faz 3).
        if !ctx.read_only {
            cmd.arg("--permission-mode").arg("acceptEdits");
        }
        if let Some(wt) = &ctx.worktree {
            cmd.current_dir(wt);
        }
        for (k, val) in &ctx.env {
            cmd.env(k, val);
        }
        cmd.stdin(Stdio::null()) // avoid the 3s stdin wait (spike S1 §2)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("claude spawn failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = ctx.span_id.clone();
        self.sessions
            .lock()
            .await
            .insert(key.clone(), ChildSession { child, stdout });
        Ok(SessionHandle {
            tool: AgentTool::Claude,
            pid,
            session_id: None, // filled once system/init is observed (by the stream task)
            task_id: key,
            started_at: divan_core::now_ms(),
        })
    }

    async fn observe(&self, handle: &SessionHandle) -> AdapterResult<EventStream> {
        let stdout = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(&handle.task_id)
                .ok_or_else(|| AdapterError::fatal("claude observe: unknown session"))?;
            session
                .stdout
                .take()
                .ok_or_else(|| AdapterError::fatal("claude observe: stdout already taken"))?
        };
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let mut sid = None;
            stream_child_stdout(stdout, tx, &mut sid).await;
        });
        Ok(rx)
    }

    async fn deliver(
        &self,
        _handle: &SessionHandle,
        _batch: MessageBatch,
    ) -> AdapterResult<DeliveryReceipt> {
        // Hook/MCP delivery is Faz 2 (spec §7). Declared but not active in Faz 1.
        Err(AdapterError::fatal(
            "claude deliver: hook/MCP delivery is Faz 2",
        ))
    }

    async fn resume(&self, session_id: &str, prompt: &str) -> AdapterResult<SessionHandle> {
        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .arg(prompt)
            .arg("-r")
            .arg(session_id)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("claude resume failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = format!("resume:{session_id}");
        self.sessions
            .lock()
            .await
            .insert(key.clone(), ChildSession { child, stdout });
        Ok(SessionHandle {
            tool: AgentTool::Claude,
            pid,
            session_id: Some(session_id.to_string()),
            task_id: key,
            started_at: divan_core::now_ms(),
        })
    }

    async fn kill(&self, handle: &SessionHandle) -> AdapterResult<()> {
        // Terminate the owned child if we still hold it.
        if let Some(mut session) = self.sessions.lock().await.remove(&handle.task_id) {
            let _ = session.child.kill().await;
        } else if let Some(pid) = handle.pid {
            // Fallback: best-effort signal (the daemon watchdog also enforces).
            let _ = Command::new("kill").arg(pid.to_string()).status().await;
        }
        Ok(())
    }
}

/// Stream a child's stdout lines into normalized events. Used by the daemon,
/// which owns the spawned `Child` (keeps stdout alive). Captures the session id
/// from the first `system/init` line into `session_id_out`.
pub async fn stream_child_stdout(
    stdout: impl tokio::io::AsyncRead + Unpin,
    tx: tokio::sync::mpsc::Sender<NormalizedEvent>,
    session_id_out: &mut Option<String>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if session_id_out.is_none() {
            if let Some(sid) = extract_session_id(&line) {
                *session_id_out = Some(sid);
            }
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

    // Fixtures captured live in spike S1 (docs/spikes/raw/s1_claude_stream_json.txt).
    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"d6f71f1a-9078","model":"claude-opus-4-8"}"#;
    const TURN: &str =
        r#"{"type":"system","subtype":"post_turn_summary","status_category":"review_ready"}"#;
    const WRITE: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/tmp/hello.txt","content":"divan"}}]}}"#;
    const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"HELLO_DIVAN","usage":{"input_tokens":2181,"output_tokens":12,"cache_read_input_tokens":5}}"#;
    const RESULT_ERR: &str =
        r#"{"type":"result","subtype":"error_max_turns","is_error":true,"usage":{}}"#;

    #[test]
    fn extracts_session_id_from_init() {
        assert_eq!(extract_session_id(INIT).as_deref(), Some("d6f71f1a-9078"));
        assert_eq!(extract_session_id(WRITE), None);
    }

    #[test]
    fn turn_summary_is_turn_end() {
        assert_eq!(parse_line(TURN), vec![NormalizedEvent::TurnEnd]);
    }

    #[test]
    fn write_tool_yields_fileedit_then_toolcall() {
        let evs = parse_line(WRITE);
        assert_eq!(evs.len(), 2);
        assert!(matches!(
            &evs[0],
            NormalizedEvent::FileEdit { path, kind }
                if path == "/tmp/hello.txt" && *kind == FileChangeKind::Added
        ));
        assert!(matches!(&evs[1], NormalizedEvent::ToolCall { name, .. } if name == "Write"));
    }

    #[test]
    fn result_success_session_end_ok_with_usage_and_text() {
        let evs = parse_line(RESULT_OK);
        assert_eq!(
            evs,
            vec![NormalizedEvent::SessionEnd {
                ok: true,
                usage: Usage {
                    input_tokens: 2181,
                    output_tokens: 12,
                    cache_read_tokens: 5,
                },
                final_text: Some("HELLO_DIVAN".into()),
            }]
        );
    }

    #[test]
    fn result_error_session_end_not_ok() {
        assert_eq!(
            parse_line(RESULT_ERR),
            vec![NormalizedEvent::SessionEnd {
                ok: false,
                usage: Usage::default(),
                final_text: None,
            }]
        );
    }

    #[test]
    fn malformed_line_is_fatal_error_event() {
        let evs = parse_line("{not json");
        assert!(matches!(
            evs.as_slice(),
            [NormalizedEvent::Error { class, .. }]
                if *class == divan_core::AgentErrorClass::Fatal
        ));
    }

    #[test]
    fn unknown_line_is_ignored() {
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
    }

    /// Real-CLI contract test (impl plan §10.3): off by default, run with
    /// `DIVAN_TEST_CLAUDE=1 cargo test -p divan-adapters real_claude`.
    #[tokio::test]
    async fn real_claude_session_emits_session_end() {
        if std::env::var("DIVAN_TEST_CLAUDE").is_err() {
            eprintln!("skip: set DIVAN_TEST_CLAUDE=1 to run the real claude contract test");
            return;
        }
        use crate::SpawnCtx;
        let adapter = ClaudeAdapter::new("claude-1");
        let ctx = SpawnCtx {
            worktree: Some(std::env::temp_dir()),
            env: std::collections::HashMap::new(),
            trace_id: "t".into(),
            span_id: "real-claude".into(),
            artifact_refs: vec![],
            read_only: true,
            prompt: "Reply with exactly the text: HELLO_DIVAN. Do not use any tools.".into(),
            max_turns: Some(1),
        };
        let handle = adapter.spawn(ctx).await.expect("spawn claude");
        let mut rx = adapter.observe(&handle).await.expect("observe claude");
        let mut ended_ok = None;
        while let Some(ev) = rx.recv().await {
            if let NormalizedEvent::SessionEnd { ok, .. } = ev {
                ended_ok = Some(ok);
            }
        }
        assert_eq!(ended_ok, Some(true), "claude session ended successfully");
    }
}

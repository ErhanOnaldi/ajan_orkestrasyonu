//! Antigravity (`agy`) CLI adapter — degraded one-shot (spec §3.5, spike S5).
//!
//! Spawn: `agy -p <prompt>` with `current_dir(worktree)`, `stdin(/dev/null)`,
//! stdout+stderr piped, default `--print-timeout` (5min). Output is PLAIN TEXT
//! (no structured stream; `--output-format json` errors — spike S5 §3). The
//! adapter therefore captures stdout as `final_text` and derives `SessionEnd`
//! from the child's process exit; there are NO reliable `FileEdit`/`ToolCall`
//! events from agy's output.
//!
//! ## CRITICAL BLOCKER (issue #7) — degraded, single-shot, stateless
//! `agy -p` does NOT return the conversation id via any supported contract
//! (stdout/stderr/file), and `-c/--continue` only continues the machine's
//! *global last* conversation — so multi-session resume is impossible
//! (spike S5 §3). Per spec §3.5 the v1 adapter is **single-shot and stateless**:
//! - `resume()` always returns `Unsupported` (a `Fatal` `AdapterError`).
//! - The card declares `multi_turn = false`, so the Cost Router never assigns
//!   agy any multi-turn work.
//! - Only one-shot task kinds (`review`, `research`, `analyze`, `report`) are
//!   accepted; the daemon/router gates this via [`AgyAdapter::accepts_kind`].
//!
//! Security: like codex (spike S2 §2) the adapter NEVER passes a permission /
//! sandbox bypass flag (no `--dangerously-skip-permissions`). Isolation = the
//! tool's own sandbox/permissions + the worktree path.

use crate::{
    AdapterError, AdapterResult, AgentAdapter, DeliveryReceipt, EventStream, MessageBatch,
    SessionHandle, SpawnCtx,
};
use async_trait::async_trait;
use divan_core::Usage;
use divan_core::{
    AgentCard, AgentId, AgentStatus, AgentTool, Capability, DeliveryKind, NormalizedEvent, TaskKind,
};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

/// A live child session owned by the adapter between `spawn` and `observe`.
struct ChildSession {
    child: Child,
    stdout: Option<ChildStdout>,
}

/// The Antigravity (`agy`) adapter — degraded one-shot reviewer/researcher.
///
/// Like [`crate::codex::CodexAdapter`] it owns spawned children and derives
/// `SessionEnd{ok}` from the child's process exit (agy emits no structured
/// stream). Unlike codex it is **stateless**: it captures plain-text stdout as
/// `final_text` and cannot resume (issue #7, spec §3.5).
pub struct AgyAdapter {
    pub id: AgentId,
    sessions: Arc<Mutex<HashMap<String, ChildSession>>>,
}

impl AgyAdapter {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(id),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// True only for the stateless one-shot task kinds agy can serve
    /// (spec §3.5): `Review | Research | Analyze | Report`.
    ///
    /// Multi-turn task kinds (`Implement`, `Test`, `Plan`) are excluded by the
    /// Cost Router via the card's `multi_turn = false`; this gate is the
    /// adapter-level check the daemon/router uses before assigning any task,
    /// since [`SpawnCtx`] does not carry the task kind directly.
    pub fn accepts_kind(kind: TaskKind) -> bool {
        matches!(
            kind,
            TaskKind::Review | TaskKind::Research | TaskKind::Analyze | TaskKind::Report
        )
    }
}

#[async_trait]
impl AgentAdapter for AgyAdapter {
    fn card(&self) -> AgentCard {
        AgentCard {
            id: self.id.clone(),
            tool: AgentTool::Agy,
            display_name: Some("Antigravity".into()),
            // One-shot reviewer/researcher: read-only capability (spec §3.5).
            capabilities: vec![Capability::Read],
            cost_class: 2,
            skills: vec![
                "review".into(),
                "research".into(),
                "analyze".into(),
                "report".into(),
            ],
            // Plain-text one-shot result is surfaced via MCP delivery only.
            delivery: vec![DeliveryKind::Mcp],
            // KEY FIELD: single-shot only. The Cost Router uses this to exclude
            // agy from any multi-turn task (issue #7, spec §3.5).
            multi_turn: false,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: divan_core::now_ms(),
        }
    }

    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle> {
        // `agy -p <prompt>` runs one prompt and prints the answer as plain text
        // (spike S5 §3). `--print-timeout` is left at its 5min default.
        // SECURITY: never pass `--dangerously-skip-permissions` (mirror codex).
        let mut cmd = Command::new("agy");
        cmd.arg("-p").arg(&ctx.prompt);
        if let Some(wt) = &ctx.worktree {
            cmd.current_dir(wt).arg("--add-dir").arg(wt);
        }
        // Optional model override.
        if let Some(model) = ctx.env.get("AGY_MODEL") {
            cmd.arg("--model").arg(model);
        }
        for (k, val) in &ctx.env {
            cmd.env(k, val);
        }
        cmd.stdin(Stdio::null()) // one-shot: never block on stdin
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("agy spawn failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = ctx.span_id.clone();
        self.sessions
            .lock()
            .await
            .insert(key.clone(), ChildSession { child, stdout });
        Ok(SessionHandle {
            tool: AgentTool::Agy,
            pid,
            // agy never returns a usable conversation id (issue #7): no resume key.
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
                .ok_or_else(|| AdapterError::fatal("agy observe: unknown session"))?;
            session.stdout.take()
        };
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sessions = self.sessions.clone();
        let key = handle.task_id.clone();
        tokio::spawn(async move {
            // agy has NO structured stream: read the whole plain-text answer
            // and keep it as `final_text` (review/research output, spike S5 §3).
            // There are no reliable FileEdit/ToolCall events to emit.
            let final_text = match stdout {
                Some(stdout) => read_all_stdout(stdout).await,
                None => None,
            };
            // agy emits no session-end marker: derive ok from the process exit.
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
        Err(AdapterError::fatal("agy deliver: not supported"))
    }

    async fn resume(&self, _session_id: &str, _prompt: &str) -> AdapterResult<SessionHandle> {
        // spec §3.5 decision: agy cannot resume a specific session — `agy -p`
        // returns no conversation id (issue #7), and `-c/--continue` only
        // continues the machine's global-last conversation. Declared Unsupported.
        Err(AdapterError::fatal(
            "agy resume is Unsupported (issue #7: no conversation id captured)",
        ))
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

/// Read a child's entire stdout into a single `final_text` string (agy prints
/// the answer as one plain-text blob; spike S5 §3). Empty output yields `None`.
async fn read_all_stdout(stdout: impl tokio::io::AsyncRead + Unpin) -> Option<String> {
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    if reader.read_to_string(&mut buf).await.is_err() {
        return None;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_kind() {
        // One-shot kinds agy serves (spec §3.5).
        assert!(AgyAdapter::accepts_kind(TaskKind::Review));
        assert!(AgyAdapter::accepts_kind(TaskKind::Research));
        assert!(AgyAdapter::accepts_kind(TaskKind::Analyze));
        assert!(AgyAdapter::accepts_kind(TaskKind::Report));
        // Multi-turn / non-one-shot kinds are excluded.
        assert!(!AgyAdapter::accepts_kind(TaskKind::Implement));
        assert!(!AgyAdapter::accepts_kind(TaskKind::Test));
        assert!(!AgyAdapter::accepts_kind(TaskKind::Plan));
    }

    #[test]
    fn card_is_single_shot() {
        let card = AgyAdapter::new("agy-1").card();
        // KEY: the router uses multi_turn=false to exclude agy from multi-turn.
        assert!(!card.multi_turn);
        assert_eq!(card.tool, AgentTool::Agy);
    }

    #[tokio::test]
    async fn resume_is_unsupported() {
        // resume never spawns; it always declares Unsupported (issue #7).
        let adapter = AgyAdapter::new("agy-1");
        let res = adapter.resume("anything", "hello").await;
        assert!(res.is_err());
        assert_eq!(
            res.err().map(|e| e.class),
            Some(divan_core::AgentErrorClass::Fatal)
        );
    }

    /// Real-CLI contract test (impl plan §10.3): off by default, run with
    /// `DIVAN_TEST_AGY=1 cargo test -p divan-adapters agy::`.
    #[tokio::test]
    async fn real_agy_session_emits_session_end_with_final_text() {
        if std::env::var("DIVAN_TEST_AGY").is_err() {
            eprintln!("skip: set DIVAN_TEST_AGY=1 to run the real agy contract test");
            return;
        }
        let adapter = AgyAdapter::new("agy-1");
        let ctx = SpawnCtx {
            worktree: Some(std::env::temp_dir()),
            env: std::collections::HashMap::new(),
            trace_id: "t".into(),
            span_id: "real-agy".into(),
            artifact_refs: vec![],
            read_only: true,
            prompt: "Reply with exactly the text: HELLO_DIVAN".into(),
            max_turns: Some(1),
        };
        let handle = adapter.spawn(ctx).await.expect("spawn agy");
        let mut rx = adapter.observe(&handle).await.expect("observe agy");
        let mut ended_ok = None;
        let mut saw_text = false;
        while let Some(ev) = rx.recv().await {
            if let NormalizedEvent::SessionEnd { ok, final_text, .. } = ev {
                ended_ok = Some(ok);
                saw_text = final_text.is_some();
            }
        }
        assert_eq!(ended_ok, Some(true), "agy session ended successfully");
        assert!(saw_text, "agy plain-text answer captured as final_text");
    }
}

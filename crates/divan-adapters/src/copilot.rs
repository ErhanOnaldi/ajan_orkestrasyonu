//! Copilot CLI adapter (spec §3.4, spike S5).
//!
//! Spawn: `copilot -p "<prompt>" --allow-tool <pattern>… [--model <m>]` with
//! `current_dir = worktree` and `stdin = /dev/null` (spike S5: avoid the stdin
//! hang). Copilot's programmatic mode is **plain text**, NOT stream-json
//! (spike S5 §3, §3.4 confirmed). Consequences:
//!
//! - **`FileEdit` events are derived from the worktree git diff**, not from the
//!   process output (stderr `Changes +N -M` is only a coarse counter with no
//!   paths). After the process exits we run `git -C <worktree> status
//!   --porcelain` and emit one `FileEdit` per changed path.
//! - **`ToolCall` granularity is unavailable in v1** (§3.4): copilot prints
//!   human-readable `● …` tool lines on stderr, not machine-readable JSON. We
//!   emit a single coarse synthetic `ToolCall{name:"copilot"}` carrying a
//!   `tool_call_unavailable` note so traces document the limitation, then rely
//!   on the diff for the real file-level signal.
//! - The plain stdout text is captured as `SessionEnd.final_text` so reviews /
//!   results are preserved (mirrors codex's `agent_message` capture).
//!
//! **`--allow-tool` ↔ Policy Engine (K8)** is the F3.4 headline: copilot is the
//! only adapter where capabilities compile to *native* tool-side permission
//! filters. [`compile_allow_tools`] is `pub` so the daemon / `policy explain`
//! can surface the exact compiled arguments (F3.4 acceptance).

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
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::Mutex;

/// Compile a task's write capability into copilot `--allow-tool` argument values
/// (spike S5 §3, §3.4 — the F3.4 headline feature).
///
/// This is `pub` so the daemon / `policy explain` can show the exact compiled
/// arguments (F3.4 acceptance: "`--allow-tool` arguments visible in
/// router/policy explain output"). The mapping:
///
/// - `read_only == true`  → allow **nothing** (no `write`, no `shell`); the
///   session is a reviewer and may only read the worktree.
/// - `read_only == false` + worktree → `write(<worktree>/**)`: copilot's native
///   path-scoped write filter. **Verified live (2026-06-12):** copilot REFUSES a
///   write outside the pattern ("permission denied for that path"), so this is a
///   real tool-side boundary (§3.4 — K8 enforced natively), not just `current_dir`.
/// - `read_only == false` + no worktree → bare `write` (no path to bound to;
///   should not happen for a real write task, which always has a worktree).
///
/// Each returned `String` is one value to pass after a `--allow-tool` flag.
pub fn compile_allow_tools(ctx: &SpawnCtx) -> Vec<String> {
    if ctx.read_only {
        // Reviewer: no write, no shell. Copilot may only read the worktree.
        return vec![];
    }
    match &ctx.worktree {
        // Scope writes to the worktree subtree (the F3.4 boundary, verified).
        Some(wt) => vec![format!("write({}/**)", wt.display())],
        None => vec!["write".to_string()],
    }
}

/// Kind of git working-tree change, parsed from `git status --porcelain`.
fn porcelain_kind(status: &str) -> FileChangeKind {
    // `git status --porcelain` columns: XY <path>. We look at the combined
    // staged/unstaged status. Deletions take priority, then additions
    // (untracked `??` or `A`), everything else is a modification.
    if status.contains('D') {
        FileChangeKind::Deleted
    } else if status.contains('A') || status.contains('?') {
        FileChangeKind::Added
    } else {
        FileChangeKind::Modified
    }
}

/// Derive the set of changed files in `worktree` from `git status --porcelain`
/// (§3.4: copilot emits no stream, so `FileEdit` events come from the diff).
///
/// Factored out (and `pub(crate)`) so the derivation is unit-testable WITHOUT
/// spawning copilot. Returns `(path, kind)` pairs; on any git failure returns an
/// empty vec (the caller still emits `SessionEnd`).
pub(crate) fn changed_files(worktree: &Path) -> Vec<(String, FileChangeKind)> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("status")
        .arg("--porcelain")
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut changes = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        // Format: two status chars, a space, then the path. Renames show as
        // `R  old -> new`; we record the new (right-hand) path as Modified.
        let status = &line[..2];
        let rest = line[3..].trim();
        let path = match rest.split(" -> ").last() {
            Some(p) => p.to_string(),
            None => rest.to_string(),
        };
        changes.push((path, porcelain_kind(status)));
    }
    changes
}

/// A live child session owned by the adapter between `spawn` and `observe`.
struct ChildSession {
    child: Child,
    stdout: Option<ChildStdout>,
    /// The worktree the session ran in; needed to derive `FileEdit` from the
    /// git diff after the process exits.
    worktree: Option<std::path::PathBuf>,
}

/// The Copilot adapter. Like [`crate::codex::CodexAdapter`] it owns spawned
/// children; copilot emits no structured stream, so `observe` captures the
/// plain stdout as `final_text`, derives `FileEdit` events from the worktree git
/// diff, and derives `SessionEnd{ok}` from the process exit status (§3.4).
pub struct CopilotAdapter {
    pub id: AgentId,
    sessions: Arc<Mutex<HashMap<String, ChildSession>>>,
}

impl CopilotAdapter {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: AgentId::new(id),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl AgentAdapter for CopilotAdapter {
    fn card(&self) -> AgentCard {
        AgentCard {
            id: self.id.clone(),
            tool: AgentTool::Copilot,
            display_name: Some("Copilot".into()),
            capabilities: vec![Capability::Read, Capability::Write],
            cost_class: 3,
            skills: vec!["implement".into()],
            // Copilot hooks are weak (§3.4): prefer MCP push over resume; NO Hook.
            delivery: vec![DeliveryKind::Mcp],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: divan_core::now_ms(),
        }
    }

    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle> {
        let mut cmd = Command::new("copilot");
        cmd.arg("-p").arg(&ctx.prompt);
        // Compile the task's capabilities into native `--allow-tool` filters
        // (F3.4 headline). Each value gets its own `--allow-tool` flag.
        for pattern in compile_allow_tools(&ctx) {
            cmd.arg("--allow-tool").arg(pattern);
        }
        // Model pinning (Cost Router, K9) — only when explicitly provided.
        if let Some(model) = ctx.env.get("DIVAN_COPILOT_MODEL") {
            cmd.arg("--model").arg(model);
        }
        if let Some(wt) = &ctx.worktree {
            cmd.current_dir(wt);
        }
        for (k, val) in &ctx.env {
            cmd.env(k, val);
        }
        cmd.stdin(Stdio::null()) // required, else copilot blocks on stdin (spike S5)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| AdapterError::fatal(format!("copilot spawn failed: {e}")))?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let key = ctx.span_id.clone();
        self.sessions.lock().await.insert(
            key.clone(),
            ChildSession {
                child,
                stdout,
                worktree: ctx.worktree.clone(),
            },
        );
        Ok(SessionHandle {
            tool: AgentTool::Copilot,
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
                .ok_or_else(|| AdapterError::fatal("copilot observe: unknown session"))?;
            session.stdout.take()
        };
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let sessions = self.sessions.clone();
        let key = handle.task_id.clone();
        tokio::spawn(async move {
            // Copilot has no structured stream: capture the plain stdout text
            // verbatim as the final result (§3.4).
            let final_text = if let Some(stdout) = stdout {
                capture_stdout(stdout).await
            } else {
                None
            };

            // Coarse synthetic ToolCall documenting that fine-grained tool-call
            // granularity is unavailable for copilot in v1 (§3.4).
            let _ = tx
                .send(NormalizedEvent::ToolCall {
                    name: "copilot".into(),
                    input: Some(serde_json::json!({ "tool_call_unavailable": true })),
                })
                .await;

            // Reap the child to get the exit status; remove it from the
            // registry. If `kill` already removed it, treat as a failed session.
            let session = sessions.lock().await.remove(&key);
            let (ok, worktree) = match session {
                Some(mut s) => {
                    let ok = s.child.wait().await.map(|st| st.success()).unwrap_or(false);
                    (ok, s.worktree)
                }
                None => (false, None),
            };

            // Derive FileEdit events from the worktree git diff (§3.4).
            if let Some(wt) = &worktree {
                for (path, kind) in changed_files(wt) {
                    if tx
                        .send(NormalizedEvent::FileEdit { path, kind })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }

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
        Err(AdapterError::fatal(
            "copilot deliver: MCP delivery is Faz 2/3",
        ))
    }

    async fn resume(&self, _session_id: &str, _prompt: &str) -> AdapterResult<SessionHandle> {
        // Spike S5 / §3.3: copilot has a resume facility but it was not verified
        // as a reliable machine-readable contract in Faz 0; v1 prefers one-shot
        // spawns and MCP delivery (card delivery = [Mcp], NO Resume). Fail fast
        // rather than spawn an unverified continuation.
        Err(AdapterError::fatal(
            "copilot resume unverified (Faz 0 S5); v1 uses one-shot + MCP delivery",
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

/// Read a copilot child's plain-text stdout to EOF, returning it as the captured
/// final result (`None` if empty). Copilot emits no JSON, so there is nothing to
/// parse line-by-line — the whole stdout IS the result (§3.4).
async fn capture_stdout(stdout: impl tokio::io::AsyncRead + Unpin) -> Option<String> {
    let mut lines = BufReader::new(stdout).lines();
    let mut buf = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&line);
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

    fn ctx(read_only: bool) -> SpawnCtx {
        SpawnCtx {
            worktree: None,
            env: std::collections::HashMap::new(),
            trace_id: "t".into(),
            span_id: "s".into(),
            artifact_refs: vec![],
            read_only,
            prompt: "do a thing".into(),
            max_turns: Some(1),
        }
    }

    #[test]
    fn allow_tools_write_scoped_to_worktree() {
        // Write task with a worktree → path-scoped native filter (F3.4, verified).
        let mut c = ctx(false);
        c.worktree = Some(std::path::PathBuf::from("/wt/task-1"));
        assert_eq!(
            compile_allow_tools(&c),
            vec!["write(/wt/task-1/**)".to_string()]
        );
    }

    #[test]
    fn allow_tools_bare_write_without_worktree() {
        assert_eq!(compile_allow_tools(&ctx(false)), vec!["write".to_string()]);
    }

    #[test]
    fn allow_tools_none_when_read_only() {
        assert!(compile_allow_tools(&ctx(true)).is_empty());
    }

    /// Run a git command in `dir`, asserting success (test helper).
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn changed_files_derives_kinds_from_git_diff() {
        // Unique temp dir without pulling in the tempfile crate.
        let dir = std::env::temp_dir().join(format!(
            "divan_copilot_diff_{}_{}",
            std::process::id(),
            divan_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");

        // Init a repo with one committed file.
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "t@divan.test"]);
        git(&dir, &["config", "user.name", "Divan Test"]);
        std::fs::write(dir.join("keep.txt"), "original\n").expect("write keep");
        std::fs::write(dir.join("gone.txt"), "delete me\n").expect("write gone");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        // Now: modify keep.txt, delete gone.txt, add new.txt.
        std::fs::write(dir.join("keep.txt"), "changed\n").expect("modify keep");
        std::fs::remove_file(dir.join("gone.txt")).expect("rm gone");
        std::fs::write(dir.join("new.txt"), "brand new\n").expect("write new");

        let mut changes = changed_files(&dir);
        changes.sort_by(|a, b| a.0.cmp(&b.0));

        let find = |name: &str| changes.iter().find(|(p, _)| p == name).map(|(_, k)| *k);
        assert_eq!(find("keep.txt"), Some(FileChangeKind::Modified));
        assert_eq!(find("gone.txt"), Some(FileChangeKind::Deleted));
        assert_eq!(find("new.txt"), Some(FileChangeKind::Added));
        assert_eq!(changes.len(), 3, "exactly three changes: {changes:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Real-CLI contract test (impl plan §10.3): off by default, run with
    /// `DIVAN_TEST_COPILOT=1 cargo test -p divan-adapters real_copilot`.
    #[tokio::test]
    async fn real_copilot_session_emits_session_end() {
        if std::env::var("DIVAN_TEST_COPILOT").is_err() {
            eprintln!("skip: set DIVAN_TEST_COPILOT=1 to run the real copilot contract test");
            return;
        }
        let adapter = CopilotAdapter::new("copilot-1");
        let ctx = SpawnCtx {
            worktree: Some(std::env::temp_dir()),
            env: std::collections::HashMap::new(),
            trace_id: "t".into(),
            span_id: "real-copilot".into(),
            artifact_refs: vec![],
            read_only: true,
            prompt: "Reply with exactly the text: HELLO_DIVAN".into(),
            max_turns: Some(1),
        };
        let handle = adapter.spawn(ctx).await.expect("spawn copilot");
        let mut rx = adapter.observe(&handle).await.expect("observe copilot");
        let mut ended_ok = None;
        let mut saw_text = false;
        while let Some(ev) = rx.recv().await {
            if let NormalizedEvent::SessionEnd { ok, final_text, .. } = ev {
                ended_ok = Some(ok);
                saw_text = final_text.is_some();
            }
        }
        assert_eq!(ended_ok, Some(true), "copilot session ended successfully");
        assert!(saw_text, "copilot plain stdout captured as final_text");
    }
}

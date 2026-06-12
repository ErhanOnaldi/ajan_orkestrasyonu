//! Task Scheduler v1 + the `write-review` flow (spec §5.2, impl plan §F1.5).
//!
//! Deterministic orchestration only — no LLM in this path (K2). The scheduler
//! drives the task state machine via [`TaskStore::transition`], runs adapter
//! sessions with a per-task watchdog timeout (spec §3.7), retries `retryable`
//! errors with bounded backoff and fails `fatal`/`timeout` ones (spec §3.2),
//! and assembles a deterministic final report linking artifacts (the hub never
//! interprets review content — spec §5.2).

use crate::config::DaemonConfig;
use crate::worktree::WorktreeManager;
use divan_adapters::{AgentAdapter, SpawnCtx};
use divan_core::{
    AgentId, NormalizedEvent, SpanId, Task, TaskId, TaskKind, TaskState, TraceEvent,
    TraceEventKind, TraceId, TransitionReason, Usage,
};
use divan_db::{ArtifactStore, TaskStore, TraceStore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Outcome of a successfully-completed session (`ok == true`).
#[derive(Debug, Clone)]
pub struct SessionOutcome {
    pub usage: Usage,
    /// The agent's final text (review/result), if any.
    pub final_text: Option<String>,
}

/// Why a single session attempt failed.
#[derive(Debug, Clone)]
enum SessionFail {
    Retryable(String),
    Fatal(String),
    Timeout,
}

/// Errors from running the write-review flow.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("no adapter registered for agent {0}")]
    UnknownAgent(String),
    #[error("worktree: {0}")]
    Worktree(String),
    #[error("db: {0}")]
    Db(#[from] divan_db::DbError),
    #[error("implement task failed: {0}")]
    ImplementFailed(String),
    #[error("review task failed: {0}")]
    ReviewFailed(String),
}

/// The deterministic final report manifest (spec §5.2 step 9). Pure linkage of
/// artifact refs + the task state timeline; no LLM interpretation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FinalReport {
    pub trace_id: String,
    pub title: String,
    pub spec_ref: String,
    pub diff_ref: String,
    pub review_ref: String,
    pub writer: String,
    pub reviewer: String,
    pub implement_state: String,
    pub review_state: String,
    /// The artifact ref of this report itself (set after publishing; F1.9
    /// requires the CLI to surface it). Not part of the hashed body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_report_ref: Option<String>,
}

/// Shared context for the tasks created by one flow run (same trace, spec, ts).
struct FlowCtx {
    trace_id: TraceId,
    spec_ref: divan_core::ArtifactRef,
    now: i64,
}

/// What `divan diff|merge|cleanup <task-id>` needs to act on a run's worktree.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub repo: PathBuf,
    pub worktree: crate::worktree::Worktree,
}

/// The scheduler owns the stores, the worktree manager, and the adapter set.
pub struct Scheduler {
    db: divan_db::Db,
    artifacts: Arc<ArtifactStore>,
    worktrees: Arc<WorktreeManager>,
    adapters: HashMap<AgentId, Arc<dyn AgentAdapter>>,
    config: DaemonConfig,
    /// Run worktree records, keyed by both root and implement task ids so the
    /// user can pass either to `divan diff|merge|cleanup` (lost on restart —
    /// a documented v1 limitation; `.divan/runs/` persistence is later).
    runs: std::sync::Mutex<HashMap<String, RunRecord>>,
    /// Retryable-error retry budget (spec §3.2 default 2).
    max_retries: u32,
    /// Backoff base between retries (kept tiny in tests).
    retry_delay: Duration,
}

impl Scheduler {
    pub fn new(
        db: divan_db::Db,
        artifacts: Arc<ArtifactStore>,
        worktrees: Arc<WorktreeManager>,
        config: DaemonConfig,
    ) -> Self {
        Self {
            db,
            artifacts,
            worktrees,
            adapters: HashMap::new(),
            config,
            runs: std::sync::Mutex::new(HashMap::new()),
            max_retries: 2,
            retry_delay: Duration::from_millis(50),
        }
    }

    /// Test/runtime knob for retry backoff.
    pub fn with_retry(mut self, max_retries: u32, retry_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.retry_delay = retry_delay;
        self
    }

    /// Register an adapter and persist its card (agent_registered trace).
    pub fn register_adapter(&mut self, adapter: Arc<dyn AgentAdapter>) -> Result<(), FlowError> {
        use divan_db::AgentStore;
        let card = adapter.card();
        self.db.upsert_agent(&card)?;
        let _ = self.db.append_event(
            &TraceEvent::new(
                TraceId::new("registry"),
                TraceEventKind::AgentRegistered,
                divan_core::now_ms(),
            )
            .with_agent(card.id.clone())
            .with_data(serde_json::json!({"tool": card.tool.as_str()})),
        );
        self.adapters.insert(card.id, adapter);
        Ok(())
    }

    fn adapter(&self, id: &AgentId) -> Result<Arc<dyn AgentAdapter>, FlowError> {
        self.adapters
            .get(id)
            .cloned()
            .ok_or_else(|| FlowError::UnknownAgent(id.to_string()))
    }

    /// Run one session attempt: spawn, observe with watchdog timeout, map events
    /// to trace, and return the outcome or a classified failure.
    async fn run_session_once(
        &self,
        task: &Task,
        agent_id: &AgentId,
        read_only: bool,
        worktree: Option<PathBuf>,
    ) -> Result<SessionOutcome, SessionFail> {
        let adapter = self
            .adapter(agent_id)
            .map_err(|e| SessionFail::Fatal(e.to_string()))?;

        let span = SpanId::new(format!("{}:{}", task.id, agent_id));
        let ctx = SpawnCtx {
            worktree: worktree.clone(),
            env: HashMap::new(),
            trace_id: task.trace_id.to_string(),
            span_id: span.to_string(),
            artifact_refs: task.spec_ref.iter().cloned().collect(),
            read_only,
            prompt: self.prompt_for(task, read_only),
            max_turns: Some(8),
        };

        let handle = adapter
            .spawn(ctx)
            .await
            .map_err(|e| classify(e.class, e.message))?;
        self.trace(
            task,
            Some(agent_id),
            TraceEventKind::Spawn,
            serde_json::json!({
                "pid": handle.pid, "read_only": read_only,
            }),
        );

        let mut rx = adapter
            .observe(&handle)
            .await
            .map_err(|e| classify(e.class, e.message))?;

        let budget =
            Duration::from_secs(self.config.runtime_secs(task.kind, task.max_runtime_secs) as u64);

        let drain = async {
            while let Some(ev) = rx.recv().await {
                match ev {
                    NormalizedEvent::ToolCall { name, .. } => self.trace(
                        task,
                        Some(agent_id),
                        TraceEventKind::ToolCall,
                        serde_json::json!({ "name": name }),
                    ),
                    NormalizedEvent::FileEdit { path, kind } => self.trace(
                        task,
                        Some(agent_id),
                        TraceEventKind::FileEdit,
                        serde_json::json!({ "path": path, "kind": format!("{kind:?}") }),
                    ),
                    NormalizedEvent::TurnEnd => self.trace(
                        task,
                        Some(agent_id),
                        TraceEventKind::SessionEvent,
                        serde_json::json!({ "event": "turn_end" }),
                    ),
                    NormalizedEvent::SessionEnd {
                        ok,
                        usage,
                        final_text,
                    } => {
                        // A CLI that ran but reported failure is a fatal session
                        // outcome — never let it reach `done` (spec §3.2).
                        if !ok {
                            return Err(SessionFail::Fatal(
                                "session reported failure (ok=false)".into(),
                            ));
                        }
                        return Ok(SessionOutcome { usage, final_text });
                    }
                    NormalizedEvent::Error { class, message } => {
                        return Err(classify(class, message))
                    }
                    NormalizedEvent::SessionIdle => {} // Faz 2 (hook idle-wake)
                }
            }
            // Channel closed without a SessionEnd — treat as fatal.
            Err(SessionFail::Fatal("session ended without result".into()))
        };

        match tokio::time::timeout(budget, drain).await {
            Ok(res) => res,
            Err(_elapsed) => {
                let _ = adapter.kill(&handle).await;
                Err(SessionFail::Timeout)
            }
        }
    }

    /// Run a session with bounded retry on `retryable` errors (spec §3.2).
    async fn run_session(
        &self,
        task: &Task,
        agent_id: &AgentId,
        read_only: bool,
        worktree: Option<PathBuf>,
    ) -> Result<SessionOutcome, SessionFail> {
        let mut attempt = 0;
        loop {
            match self
                .run_session_once(task, agent_id, read_only, worktree.clone())
                .await
            {
                Ok(o) => return Ok(o),
                Err(SessionFail::Retryable(msg)) if attempt < self.max_retries => {
                    attempt += 1;
                    self.trace(
                        task,
                        Some(agent_id),
                        TraceEventKind::Error,
                        serde_json::json!({ "class": "retryable", "attempt": attempt, "message": msg }),
                    );
                    tokio::time::sleep(self.retry_delay).await;
                }
                Err(other) => return Err(other),
            }
        }
    }

    /// The Faz 1 `write-review` flow (spec §5.2): Claude implements, Codex
    /// reviews (read-only), the hub emits a deterministic final report.
    pub async fn run_write_review(
        &self,
        title: &str,
        spec_text: &str,
        repo: &Path,
        writer: &AgentId,
        reviewer: &AgentId,
    ) -> Result<FinalReport, FlowError> {
        let now = divan_core::now_ms();
        let trace_id = TraceId::new(format!("trace-{now}"));
        let slug = slugify(title);

        // Spec artifact (K4 pointer).
        let spec_ref = self
            .artifacts
            .put(
                spec_text.as_bytes(),
                Some("text/markdown"),
                Some("write-review spec"),
                None,
                now,
            )
            .map_err(FlowError::Db)?;

        // Root + two children (spec §5.2).
        let root_id = TaskId::new(format!("root-{now}"));
        let impl_id = TaskId::new(format!("impl-{now}"));
        let review_id = TaskId::new(format!("review-{now}"));
        let fctx = FlowCtx {
            trace_id: trace_id.clone(),
            spec_ref: spec_ref.clone(),
            now,
        };
        self.create_task(&fctx, &root_id, None, TaskKind::Plan, title)?;
        self.create_task(
            &fctx,
            &impl_id,
            Some(&root_id),
            TaskKind::Implement,
            &format!("implement: {title}"),
        )?;
        self.create_task(
            &fctx,
            &review_id,
            Some(&root_id),
            TaskKind::Review,
            &format!("review: {title}"),
        )?;
        self.db.add_dep(&review_id, &impl_id)?;

        // ---- implement ----
        let wt = self.create_worktree(repo, &impl_id, &slug).await?;
        self.trace(
            &self.must_get(&root_id)?,
            None,
            TraceEventKind::WorktreeCreated,
            serde_json::json!({
                "task_id": impl_id.as_str(), "branch": wt.branch, "path": wt.path.to_string_lossy(),
            }),
        );
        {
            let rec = RunRecord {
                repo: repo.to_path_buf(),
                worktree: wt.clone(),
            };
            let mut runs = self.runs.lock().expect("runs lock");
            runs.insert(root_id.to_string(), rec.clone());
            runs.insert(impl_id.to_string(), rec);
        }
        {
            use divan_db::AgentStore;
            self.db
                .set_agent_status(writer, divan_core::AgentStatus::Busy, None)?;
        }
        self.db.assign(&impl_id, writer, wt.path.to_str())?;
        let impl_task = self.must_get(&impl_id)?;
        self.transition(
            &impl_id,
            TaskState::Claimed,
            TransitionReason::Other("assigned".into()),
        )?;
        self.transition(
            &impl_id,
            TaskState::Working,
            TransitionReason::Other("spawn".into()),
        )?;

        if let Err(fail) = self
            .run_session(&impl_task, writer, false, Some(wt.path.clone()))
            .await
        {
            let reason = self.fail_task(&impl_id, &fail);
            // Failed implement => review never starts (impl plan §F1.5 acceptance).
            self.transition(&root_id, TaskState::Failed, TransitionReason::Fatal)
                .ok();
            self.set_idle(writer);
            return Err(FlowError::ImplementFailed(reason));
        }
        self.transition(&impl_id, TaskState::Done, TransitionReason::Completed)?;
        self.set_idle(writer);

        // Snapshot the agent's work. Agents edit files in the worktree without
        // committing, so `git diff HEAD...branch` would otherwise be empty and
        // `divan merge` would merge nothing (F1.6/F1.10). Commit ALL changes
        // (including untracked) so diff/merge operate on real content.
        self.commit_worktree(&wt, &format!("divan: implement {title}"))
            .await?;

        // Diff artifact from the implement worktree.
        let diff = self.worktree_diff(repo, &wt).await?;
        let diff_ref = self
            .artifacts
            .put(
                diff.as_bytes(),
                Some("text/x-diff"),
                Some("implement diff"),
                Some(writer),
                divan_core::now_ms(),
            )
            .map_err(FlowError::Db)?;

        // ---- review (read-only; reads the implement worktree, spec §5.6) ----
        debug_assert!(self.db.deps_satisfied(&review_id)?);
        {
            use divan_db::AgentStore;
            self.db
                .set_agent_status(reviewer, divan_core::AgentStatus::Busy, None)?;
        }
        self.db.assign(&review_id, reviewer, wt.path.to_str())?;
        let review_task = self.must_get(&review_id)?;
        self.transition(
            &review_id,
            TaskState::Claimed,
            TransitionReason::Other("assigned".into()),
        )?;
        self.transition(
            &review_id,
            TaskState::Working,
            TransitionReason::Other("spawn".into()),
        )?;

        let review_outcome = match self
            .run_session(&review_task, reviewer, true, Some(wt.path.clone()))
            .await
        {
            Ok(o) => o,
            Err(fail) => {
                let reason = self.fail_task(&review_id, &fail);
                self.transition(&root_id, TaskState::Failed, TransitionReason::Fatal)
                    .ok();
                self.set_idle(reviewer);
                return Err(FlowError::ReviewFailed(reason));
            }
        };
        self.transition(&review_id, TaskState::Done, TransitionReason::Completed)?;
        self.set_idle(reviewer);

        // Review artifact = the reviewer's actual output text (spec §5.2 "Review
        // sonucu artifact olur"). The read-only review can't write a file, so we
        // capture its final message; only if the agent produced nothing do we
        // fall back to a note (the hub never invents review content, K2).
        let review_body = review_outcome.final_text.clone().unwrap_or_else(|| {
            format!(
                "Review of '{title}' by {reviewer} produced no textual output; see trace timeline."
            )
        });
        let review_ref = self
            .artifacts
            .put(
                review_body.as_bytes(),
                Some("text/markdown"),
                Some("review result"),
                Some(reviewer),
                divan_core::now_ms(),
            )
            .map_err(FlowError::Db)?;

        // Final report manifest (deterministic linkage, spec §5.2).
        self.transition(
            &root_id,
            TaskState::Claimed,
            TransitionReason::Other("report".into()),
        )
        .ok();
        self.transition(
            &root_id,
            TaskState::Working,
            TransitionReason::Other("report".into()),
        )
        .ok();
        let mut report = FinalReport {
            trace_id: trace_id.to_string(),
            title: title.to_string(),
            spec_ref: spec_ref.to_string(),
            diff_ref: diff_ref.to_string(),
            review_ref: review_ref.to_string(),
            writer: writer.to_string(),
            reviewer: reviewer.to_string(),
            implement_state: self.must_get(&impl_id)?.state.as_str().to_string(),
            review_state: self.must_get(&review_id)?.state.as_str().to_string(),
            final_report_ref: None,
        };
        let report_json = serde_json::to_vec_pretty(&report).expect("serialize report");
        let report_ref = self
            .artifacts
            .put(
                &report_json,
                Some("application/json"),
                Some("final report manifest"),
                None,
                divan_core::now_ms(),
            )
            .map_err(FlowError::Db)?;
        report.final_report_ref = Some(report_ref.to_string());
        self.trace(
            &self.must_get(&root_id)?,
            None,
            TraceEventKind::ArtifactPublished,
            serde_json::json!({
                "kind": "final_report", "ref": report_ref.to_string(),
            }),
        );
        self.transition(&root_id, TaskState::Done, TransitionReason::Completed)?;
        Ok(report)
    }

    // ---- worktree commands (divan diff|merge|cleanup) ----

    /// Clone of the shared DB handle for read-only RPC queries (status/log).
    pub fn db(&self) -> divan_db::Db {
        self.db.clone()
    }

    /// Shared artifact store (for MCP `publish_artifact`/`get_artifact`).
    pub fn artifacts(&self) -> Arc<ArtifactStore> {
        self.artifacts.clone()
    }

    fn run_record(&self, task_id: &str) -> Result<RunRecord, FlowError> {
        self.runs
            .lock()
            .expect("runs lock")
            .get(task_id)
            .cloned()
            .ok_or_else(|| FlowError::Worktree(format!("no worktree run for task {task_id}")))
    }

    /// `divan diff <task-id>` — unified diff of the run's worktree.
    pub async fn diff_task(&self, task_id: &str) -> Result<String, FlowError> {
        let rec = self.run_record(task_id)?;
        let diff = self.worktree_diff(&rec.repo, &rec.worktree).await?;
        self.trace_for_task(
            task_id,
            TraceEventKind::WorktreeDiffed,
            serde_json::json!({
                "branch": rec.worktree.branch,
            }),
        );
        Ok(diff)
    }

    /// `divan merge <task-id>` — merge the worktree branch (explicit user action,
    /// never automatic — spec §5.6).
    pub async fn merge_task(&self, task_id: &str) -> Result<(), FlowError> {
        let rec = self.run_record(task_id)?;
        self.trace_for_task(
            task_id,
            TraceEventKind::WorktreeMergeRequested,
            serde_json::json!({
                "branch": rec.worktree.branch,
            }),
        );
        let mgr = self.worktrees.clone();
        let (repo, wt) = (rec.repo.clone(), rec.worktree.clone());
        tokio::task::spawn_blocking(move || mgr.merge(&repo, &wt))
            .await
            .map_err(|e| FlowError::Worktree(format!("join: {e}")))?
            .map_err(|e| FlowError::Worktree(e.to_string()))?;
        self.trace_for_task(
            task_id,
            TraceEventKind::WorktreeMerged,
            serde_json::json!({
                "branch": rec.worktree.branch,
            }),
        );
        Ok(())
    }

    /// `divan cleanup <task-id>` — remove ONLY the worktree; artifacts are never
    /// touched (spec §3.7, impl plan §5.6).
    pub async fn cleanup_task(&self, task_id: &str) -> Result<(), FlowError> {
        let rec = self.run_record(task_id)?;
        let mgr = self.worktrees.clone();
        let (repo, wt) = (rec.repo.clone(), rec.worktree.clone());
        tokio::task::spawn_blocking(move || mgr.cleanup(&repo, &wt))
            .await
            .map_err(|e| FlowError::Worktree(format!("join: {e}")))?
            .map_err(|e| FlowError::Worktree(e.to_string()))?;
        // Drop run records pointing at this worktree (both root and impl keys).
        let branch = rec.worktree.branch.clone();
        self.runs
            .lock()
            .expect("runs lock")
            .retain(|_, r| r.worktree.branch != branch);
        Ok(())
    }

    fn trace_for_task(&self, task_id: &str, kind: TraceEventKind, data: serde_json::Value) {
        if let Ok(Some(task)) = self.db.get_task(&TaskId::new(task_id)) {
            self.trace(&task, None, kind, data);
        }
    }

    /// Commit ALL changes (incl. untracked) in a worktree so subsequent diff/
    /// merge see the agent's edits (agents rarely commit themselves).
    async fn commit_worktree(
        &self,
        wt: &crate::worktree::Worktree,
        message: &str,
    ) -> Result<(), FlowError> {
        let mgr = self.worktrees.clone();
        let wt = wt.clone();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || mgr.commit_all(&wt, &message))
            .await
            .map_err(|e| FlowError::Worktree(format!("join: {e}")))?
            .map_err(|e| FlowError::Worktree(e.to_string()))
    }

    // ---- helpers ----

    /// Build the session prompt. The full spec is embedded inline because Faz 1
    /// has no MCP artifact retrieval yet — agents cannot fetch `spec_ref`, so a
    /// truncated title alone would silently lose the request (F1.9 correctness).
    fn prompt_for(&self, task: &Task, read_only: bool) -> String {
        let spec = self.spec_text(task);
        if read_only {
            format!(
                "You are reviewing an implementation in the current worktree (read-only).\n\
                 Review the changes against the specification below and reply with your \
                 review as your final message — findings, risks, and a verdict.\n\n\
                 ## Specification\n{spec}"
            )
        } else {
            format!(
                "Implement the following specification by editing files in the current \
                 worktree. When done, summarize what you changed as your final message.\n\n\
                 ## Specification\n{spec}"
            )
        }
    }

    /// Read the spec artifact content for a task, falling back to the title.
    fn spec_text(&self, task: &Task) -> String {
        if let Some(ref_str) = &task.spec_ref {
            if let Ok(bytes) = self
                .artifacts
                .get(&divan_core::ArtifactRef::new(ref_str.clone()))
            {
                if let Ok(text) = String::from_utf8(bytes) {
                    return text;
                }
            }
        }
        task.title.clone()
    }

    fn create_task(
        &self,
        ctx: &FlowCtx,
        id: &TaskId,
        parent: Option<&TaskId>,
        kind: TaskKind,
        title: &str,
    ) -> Result<(), FlowError> {
        let task = Task {
            id: id.clone(),
            parent_id: parent.cloned(),
            kind,
            title: title.to_string(),
            spec_ref: Some(ctx.spec_ref.to_string()),
            state: TaskState::Open,
            assignee: None,
            worktree: None,
            max_runtime_secs: None,
            trace_id: ctx.trace_id.clone(),
            created_at: ctx.now,
            updated_at: ctx.now,
        };
        self.db.create_task(&task)?;
        Ok(())
    }

    fn must_get(&self, id: &TaskId) -> Result<Task, FlowError> {
        self.db
            .get_task(id)?
            .ok_or_else(|| FlowError::Db(divan_db::DbError::NotFound(id.to_string())))
    }

    fn transition(
        &self,
        id: &TaskId,
        to: TaskState,
        reason: TransitionReason,
    ) -> Result<(), FlowError> {
        self.db.transition(id, to, reason)?;
        Ok(())
    }

    /// Map a session failure to a terminal task transition; returns the reason text.
    fn fail_task(&self, id: &TaskId, fail: &SessionFail) -> String {
        let (reason, text) = match fail {
            SessionFail::Timeout => (TransitionReason::Timeout, "timeout".to_string()),
            SessionFail::Fatal(m) => (TransitionReason::Fatal, m.clone()),
            SessionFail::Retryable(m) => {
                (TransitionReason::Fatal, format!("retries exhausted: {m}"))
            }
        };
        let _ = self.db.transition(id, TaskState::Failed, reason);
        text
    }

    fn set_idle(&self, agent: &AgentId) {
        use divan_db::AgentStore;
        let _ = self
            .db
            .set_agent_status(agent, divan_core::AgentStatus::Idle, None);
    }

    fn trace(
        &self,
        task: &Task,
        agent: Option<&AgentId>,
        kind: TraceEventKind,
        data: serde_json::Value,
    ) {
        let mut ev =
            TraceEvent::new(task.trace_id.clone(), kind, divan_core::now_ms()).with_data(data);
        if let Some(a) = agent {
            ev = ev.with_agent(a.clone());
        }
        let _ = self.db.append_event(&ev);
    }

    async fn create_worktree(
        &self,
        repo: &Path,
        task_id: &TaskId,
        slug: &str,
    ) -> Result<crate::worktree::Worktree, FlowError> {
        let wt = self.worktrees.clone();
        let repo = repo.to_path_buf();
        let id = task_id.to_string();
        let slug = slug.to_string();
        let res = tokio::task::spawn_blocking(move || wt.create(&repo, &id, &slug))
            .await
            .map_err(|e| FlowError::Worktree(format!("join: {e}")))?
            .map_err(|e| FlowError::Worktree(e.to_string()))?;
        // worktree_created trace handled by caller's flow trace set.
        Ok(res)
    }

    async fn worktree_diff(
        &self,
        repo: &Path,
        wt: &crate::worktree::Worktree,
    ) -> Result<String, FlowError> {
        let mgr = self.worktrees.clone();
        let repo = repo.to_path_buf();
        let wt = wt.clone();
        tokio::task::spawn_blocking(move || mgr.diff(&repo, &wt))
            .await
            .map_err(|e| FlowError::Worktree(format!("join: {e}")))?
            .map_err(|e| FlowError::Worktree(e.to_string()))
    }
}

fn classify(class: divan_core::AgentErrorClass, message: String) -> SessionFail {
    match class {
        divan_core::AgentErrorClass::Retryable => SessionFail::Retryable(message),
        divan_core::AgentErrorClass::Fatal => SessionFail::Fatal(message),
    }
}

/// Slugify a title into a branch-safe slug.
fn slugify(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    let collapsed = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    collapsed.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_adapters::fake::FakeAdapter;
    use divan_core::AgentTool;

    fn git(args: &[&str], dir: &Path) {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git");
        assert!(
            st.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn temp_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        git(&["init", "-q", "-b", "main"], d.path());
        git(&["config", "user.email", "t@divan.dev"], d.path());
        git(&["config", "user.name", "Divan Test"], d.path());
        std::fs::write(d.path().join("README.md"), "base\n").unwrap();
        git(&["add", "."], d.path());
        git(&["commit", "-qm", "init"], d.path());
        d
    }

    fn scheduler(db: divan_db::Db, art_root: &Path, wt_root: &Path) -> Scheduler {
        let artifacts = Arc::new(ArtifactStore::new(db.clone(), art_root));
        let worktrees = Arc::new(WorktreeManager::new(wt_root));
        let cfg = DaemonConfig::default_for_home(Path::new("/tmp"));
        Scheduler::new(db, artifacts, worktrees, cfg).with_retry(2, Duration::from_millis(1))
    }

    #[tokio::test]
    async fn write_review_happy_path_to_done() {
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        s.register_adapter(Arc::new(FakeAdapter::completing("claude-1", vec![])))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();

        let report = s
            .run_write_review(
                "Add greeting",
                "implement a greeting",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap();
        assert_eq!(report.implement_state, "done");
        assert_eq!(report.review_state, "done");
        assert!(!report.diff_ref.is_empty() && !report.review_ref.is_empty());
    }

    #[tokio::test]
    async fn failed_implement_blocks_review() {
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        s.register_adapter(Arc::new(FakeAdapter::failing("claude-1", "boom")))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();

        let err = s
            .run_write_review(
                "X",
                "spec",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::ImplementFailed(_)));
        // Review task must still be Open (never started).
        let tasks = db.list_tasks().unwrap();
        let review = tasks.iter().find(|t| t.kind == TaskKind::Review).unwrap();
        assert_eq!(review.state, TaskState::Open);
        let impl_t = tasks
            .iter()
            .find(|t| t.kind == TaskKind::Implement)
            .unwrap();
        assert_eq!(impl_t.state, TaskState::Failed);
    }

    #[tokio::test]
    async fn hanging_session_times_out_to_failed() {
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        // Tiny budget so the watchdog fires fast.
        let mut cfg = DaemonConfig::default_for_home(Path::new("/tmp"));
        cfg.runtime_defaults.implement_secs = 0; // 0s budget => immediate timeout
        let artifacts = Arc::new(ArtifactStore::new(db.clone(), repo.path()));
        let worktrees = Arc::new(WorktreeManager::new(repo.path().join(".divan/wt")));
        let mut s2 = Scheduler::new(db.clone(), artifacts, worktrees, cfg);
        s2.register_adapter(Arc::new(FakeAdapter::hanging("claude-1")))
            .unwrap();
        s2.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();
        let _ = &mut s;

        let err = s2
            .run_write_review(
                "Hang",
                "spec",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::ImplementFailed(_)));
        let tasks = db.list_tasks().unwrap();
        let impl_t = tasks
            .iter()
            .find(|t| t.kind == TaskKind::Implement)
            .unwrap();
        assert_eq!(impl_t.state, TaskState::Failed);
        // Trace records the timeout reason.
        let tl = db.timeline(&impl_t.trace_id).unwrap();
        assert!(tl.iter().any(|e| e
            .data
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.as_str())
            == Some("timeout")));
    }

    #[tokio::test]
    async fn session_ok_false_marks_task_failed_not_done() {
        // A CLI that ran but reported failure (ok=false) must fail the task,
        // never reach `done` (spec §3.2).
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        s.register_adapter(Arc::new(FakeAdapter::ending_not_ok("claude-1")))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();

        let err = s
            .run_write_review(
                "X",
                "spec",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::ImplementFailed(_)));
        let tasks = db.list_tasks().unwrap();
        let impl_t = tasks
            .iter()
            .find(|t| t.kind == TaskKind::Implement)
            .unwrap();
        assert_eq!(impl_t.state, TaskState::Failed, "ok=false => failed");
        let review = tasks.iter().find(|t| t.kind == TaskKind::Review).unwrap();
        assert_eq!(review.state, TaskState::Open, "review never started");
    }

    #[tokio::test]
    async fn retryable_errors_are_retried_then_succeed() {
        // Writer fails retryably twice, then succeeds; with max_retries=2 the
        // flow completes (spec §3.2 bounded backoff).
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        s.register_adapter(Arc::new(FakeAdapter::retryable_then_ok("claude-1", 2)))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();

        let report = s
            .run_write_review(
                "Retry",
                "spec",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap();
        assert_eq!(report.implement_state, "done");
        // The two retry attempts are traced as retryable errors.
        let impl_t = db
            .list_tasks()
            .unwrap()
            .into_iter()
            .find(|t| t.kind == TaskKind::Implement)
            .unwrap();
        let retries = db
            .timeline(&impl_t.trace_id)
            .unwrap()
            .into_iter()
            .filter(|e| {
                e.event == TraceEventKind::Error
                    && e.data
                        .as_ref()
                        .and_then(|d| d.get("class"))
                        .and_then(|c| c.as_str())
                        == Some("retryable")
            })
            .count();
        assert_eq!(retries, 2, "exactly two retryable attempts traced");
    }

    #[tokio::test]
    async fn retryable_errors_exhaust_then_fail() {
        // Three retryable failures with max_retries=2 => the task fails.
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let mut s = scheduler(db.clone(), repo.path(), &repo.path().join(".divan/wt"));
        s.register_adapter(Arc::new(FakeAdapter::retryable_then_ok("claude-1", 99)))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing("codex-1", vec![]).with_tool(AgentTool::Codex),
        ))
        .unwrap();
        let err = s
            .run_write_review(
                "Exhaust",
                "spec",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FlowError::ImplementFailed(_)));
    }

    #[tokio::test]
    async fn review_artifact_contains_reviewers_final_text() {
        // The review artifact must be the reviewer's actual output (spec §5.2),
        // not a generic fallback note.
        let repo = temp_repo();
        let db = divan_db::Db::open_in_memory().unwrap();
        let art_root = repo.path().join(".divan/art");
        let artifacts = Arc::new(ArtifactStore::new(db.clone(), &art_root));
        let worktrees = Arc::new(WorktreeManager::new(repo.path().join(".divan/wt")));
        let cfg = DaemonConfig::default_for_home(Path::new("/tmp"));
        let mut s = Scheduler::new(db.clone(), artifacts.clone(), worktrees, cfg)
            .with_retry(2, Duration::from_millis(1));
        s.register_adapter(Arc::new(FakeAdapter::completing("claude-1", vec![])))
            .unwrap();
        s.register_adapter(Arc::new(
            FakeAdapter::completing_with_text("codex-1", "REVIEW: ships it, one nit on naming.")
                .with_tool(AgentTool::Codex),
        ))
        .unwrap();

        let report = s
            .run_write_review(
                "Add greeting",
                "implement a greeting",
                repo.path(),
                &AgentId::new("claude-1"),
                &AgentId::new("codex-1"),
            )
            .await
            .unwrap();
        let body = artifacts
            .get(&divan_core::ArtifactRef::new(report.review_ref.clone()))
            .unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "REVIEW: ships it, one nit on naming."
        );
        assert!(
            report.final_report_ref.is_some(),
            "final report ref surfaced"
        );
    }

    #[test]
    fn slugify_is_branch_safe() {
        assert_eq!(slugify("Add  Greeting! 123"), "add-greeting-123");
        assert_eq!(slugify("  --x--  "), "x");
    }
}

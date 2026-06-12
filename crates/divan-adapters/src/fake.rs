//! Deterministic in-memory adapter for scheduler/integration tests
//! (impl plan §F1.5, §10.2). No external processes; emits a scripted body of
//! events followed by a configurable ending so flow, retry, failure, timeout,
//! and result-capture logic can be tested without a real CLI.

use crate::{
    AdapterError, AdapterResult, AgentAdapter, DeliveryReceipt, EventStream, MessageBatch,
    SessionHandle, SpawnCtx,
};
use async_trait::async_trait;
use divan_core::{
    AgentCard, AgentId, AgentStatus, AgentTool, Capability, DeliveryKind, NormalizedEvent,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// How an observed fake session ends.
#[derive(Clone)]
enum Ending {
    /// `SessionEnd{ok:true}` carrying optional final text (the review/result).
    Ok(Option<String>),
    /// `SessionEnd{ok:false}` — the CLI reported failure (spec §3.2).
    NotOk,
    /// A fatal `Error` event mid-run.
    Fatal(String),
    /// Emit a retryable `Error` for the first `n` observes, then succeed
    /// (exercises the scheduler's bounded-retry policy, spec §3.2).
    RetryableThenOk(usize),
    /// Never ends — holds the channel open so the watchdog timeout fires.
    Hang,
}

/// A fake adapter replaying a body of events plus a configurable [`Ending`].
#[derive(Clone)]
pub struct FakeAdapter {
    id: AgentId,
    tool: AgentTool,
    multi_turn: bool,
    body: Arc<Vec<NormalizedEvent>>,
    ending: Ending,
    fail_spawn: bool,
    resume_unsupported: bool,
    /// Observe-attempt counter (drives `RetryableThenOk`).
    attempts: Arc<AtomicUsize>,
    /// Tracks the most recent batch delivered (for delivery assertions).
    pub last_batch: Arc<Mutex<Option<MessageBatch>>>,
}

impl FakeAdapter {
    fn build(id: impl Into<String>, body: Vec<NormalizedEvent>, ending: Ending) -> Self {
        Self {
            id: AgentId::new(id),
            tool: AgentTool::Claude,
            multi_turn: true,
            body: Arc::new(body),
            ending,
            fail_spawn: false,
            resume_unsupported: false,
            attempts: Arc::new(AtomicUsize::new(0)),
            last_batch: Arc::new(Mutex::new(None)),
        }
    }

    /// Completes successfully: emits `body` then `SessionEnd{ok:true}`.
    pub fn completing(id: impl Into<String>, body: Vec<NormalizedEvent>) -> Self {
        Self::build(id, body, Ending::Ok(None))
    }

    /// Completes successfully and reports `final_text` (the agent's result —
    /// e.g. the reviewer's review, captured into the review artifact).
    pub fn completing_with_text(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self::build(id, vec![], Ending::Ok(Some(text.into())))
    }

    /// Ends with `SessionEnd{ok:false}` (CLI ran but reported failure).
    pub fn ending_not_ok(id: impl Into<String>) -> Self {
        Self::build(id, vec![], Ending::NotOk)
    }

    /// Never ends (watchdog `failed(timeout)` tests).
    pub fn hanging(id: impl Into<String>) -> Self {
        Self::build(
            id,
            vec![NormalizedEvent::ToolCall {
                name: "noop".into(),
                input: None,
            }],
            Ending::Hang,
        )
    }

    /// Fails fatally mid-run (no retry, spec §3.2).
    pub fn failing(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::build(id, vec![], Ending::Fatal(message.into()))
    }

    /// Fails retryably for the first `n` attempts, then succeeds (retry policy).
    pub fn retryable_then_ok(id: impl Into<String>, n: usize) -> Self {
        Self::build(id, vec![], Ending::RetryableThenOk(n))
    }

    pub fn with_tool(mut self, tool: AgentTool) -> Self {
        self.tool = tool;
        self
    }
    pub fn with_multi_turn(mut self, multi_turn: bool) -> Self {
        self.multi_turn = multi_turn;
        self
    }
    pub fn with_fail_spawn(mut self, fail: bool) -> Self {
        self.fail_spawn = fail;
        self
    }
    pub fn with_resume_unsupported(mut self, v: bool) -> Self {
        self.resume_unsupported = v;
        self
    }
}

#[async_trait]
impl AgentAdapter for FakeAdapter {
    fn card(&self) -> AgentCard {
        AgentCard {
            id: self.id.clone(),
            tool: self.tool,
            display_name: Some(format!("fake:{}", self.id)),
            capabilities: vec![Capability::Read, Capability::Write],
            cost_class: 1,
            skills: vec![],
            delivery: vec![DeliveryKind::Mcp],
            multi_turn: self.multi_turn,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 0,
        }
    }

    async fn spawn(&self, ctx: SpawnCtx) -> AdapterResult<SessionHandle> {
        if self.fail_spawn {
            return Err(AdapterError::fatal("fake: spawn failure"));
        }
        Ok(SessionHandle {
            tool: self.tool,
            pid: Some(424242),
            session_id: Some(format!("fake-session-{}", self.id)),
            task_id: ctx.span_id,
            started_at: 0,
        })
    }

    async fn observe(&self, _handle: &SessionHandle) -> AdapterResult<EventStream> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        let body = self.body.clone();
        let ending = self.ending.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(body.len() + 2);
        tokio::spawn(async move {
            for ev in body.iter() {
                if tx.send(ev.clone()).await.is_err() {
                    return;
                }
            }
            let end = match ending {
                Ending::Ok(text) => NormalizedEvent::SessionEnd {
                    ok: true,
                    usage: Default::default(),
                    final_text: text,
                },
                Ending::NotOk => NormalizedEvent::SessionEnd {
                    ok: false,
                    usage: Default::default(),
                    final_text: None,
                },
                Ending::Fatal(msg) => NormalizedEvent::fatal(msg),
                Ending::RetryableThenOk(n) => {
                    if attempt < n {
                        NormalizedEvent::retryable(format!("fake transient (attempt {attempt})"))
                    } else {
                        NormalizedEvent::SessionEnd {
                            ok: true,
                            usage: Default::default(),
                            final_text: None,
                        }
                    }
                }
                Ending::Hang => {
                    std::future::pending::<()>().await;
                    return;
                }
            };
            let _ = tx.send(end).await;
        });
        Ok(rx)
    }

    async fn deliver(
        &self,
        _handle: &SessionHandle,
        batch: MessageBatch,
    ) -> AdapterResult<DeliveryReceipt> {
        *self.last_batch.lock().await = Some(batch);
        Ok(DeliveryReceipt {
            accepted: true,
            injected_at: 0,
            raw_status: Some("fake".into()),
        })
    }

    async fn resume(&self, session_id: &str, _prompt: &str) -> AdapterResult<SessionHandle> {
        if self.resume_unsupported {
            return Err(AdapterError::fatal("fake: resume unsupported"));
        }
        Ok(SessionHandle {
            tool: self.tool,
            pid: Some(424242),
            session_id: Some(session_id.to_string()),
            task_id: String::new(),
            started_at: 0,
        })
    }

    async fn kill(&self, _handle: &SessionHandle) -> AdapterResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx() -> SpawnCtx {
        SpawnCtx {
            worktree: None,
            env: HashMap::new(),
            trace_id: "t".into(),
            span_id: "s".into(),
            artifact_refs: vec![],
            read_only: false,
            prompt: "do it".into(),
            max_turns: Some(1),
        }
    }

    async fn drain(fake: &FakeAdapter) -> Vec<NormalizedEvent> {
        let h = fake.spawn(ctx()).await.unwrap();
        let mut rx = fake.observe(&h).await.unwrap();
        let mut out = vec![];
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn completing_emits_session_end_ok() {
        let evs = drain(&FakeAdapter::completing("c1", vec![])).await;
        assert_eq!(
            evs.last(),
            Some(&NormalizedEvent::SessionEnd {
                ok: true,
                usage: Default::default(),
                final_text: None
            })
        );
    }

    #[tokio::test]
    async fn completing_with_text_carries_final_text() {
        let evs = drain(&FakeAdapter::completing_with_text("c1", "LGTM, minor nits")).await;
        assert!(matches!(
            evs.last(),
            Some(NormalizedEvent::SessionEnd { final_text: Some(t), .. }) if t == "LGTM, minor nits"
        ));
    }

    #[tokio::test]
    async fn ending_not_ok_reports_failure() {
        let evs = drain(&FakeAdapter::ending_not_ok("c1")).await;
        assert!(matches!(
            evs.last(),
            Some(NormalizedEvent::SessionEnd { ok: false, .. })
        ));
    }

    #[tokio::test]
    async fn fail_spawn_is_fatal() {
        let fake = FakeAdapter::completing("c1", vec![]).with_fail_spawn(true);
        let err = fake.spawn(ctx()).await.unwrap_err();
        assert_eq!(err.class, divan_core::AgentErrorClass::Fatal);
    }

    #[tokio::test]
    async fn resume_unsupported_is_fatal() {
        let fake = FakeAdapter::completing("agy", vec![])
            .with_tool(AgentTool::Agy)
            .with_resume_unsupported(true);
        assert!(fake.resume("x", "p").await.is_err());
    }

    #[tokio::test]
    async fn retryable_then_ok_switches_after_n_attempts() {
        let fake = FakeAdapter::retryable_then_ok("c1", 2);
        // First two observes are retryable, third succeeds.
        let h = fake.spawn(ctx()).await.unwrap();
        for _ in 0..2 {
            let mut rx = fake.observe(&h).await.unwrap();
            let ev = rx.recv().await.unwrap();
            assert!(matches!(
                ev,
                NormalizedEvent::Error {
                    class: divan_core::AgentErrorClass::Retryable,
                    ..
                }
            ));
        }
        let mut rx = fake.observe(&h).await.unwrap();
        assert!(matches!(
            rx.recv().await.unwrap(),
            NormalizedEvent::SessionEnd { ok: true, .. }
        ));
    }
}

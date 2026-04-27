//! Task-10 (Phase 6b) wiring tests — the Task-6 triad on `HarnessDeps`.
//!
//! Covers three behaviours inherited from the retiring `AgentLoop`:
//!   1. **Budget — `FinalReply`** trips `hit_limit` and short-circuits to
//!      `TurnState::Done` without invoking the LLM.
//!   2. **Budget — `CompactAndContinue`** invokes the attached
//!      `ContextCompactor` before the LLM call.
//!   3. **Stop-hook veto** forces one additional `TurnState::Continue`
//!      even when the LLM produced no tool calls.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio_util::sync::CancellationToken;

use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::error::Result as AlephResult;
use crate::harness::{AgentHarness, Harness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::sandbox::SandboxOutput;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};
use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};

// -- Minimal mocks (kept local; the think/act suites have larger copies) -----

#[derive(Default)]
struct MockSessionInner {
    events: Vec<SessionEventRecord>,
    next_seq: EventSeq,
}

struct MockSession {
    inner: AsyncMutex<MockSessionInner>,
}

impl MockSession {
    fn new(initial: Vec<SessionEvent>) -> Arc<Self> {
        let mut inner = MockSessionInner::default();
        for event in initial {
            let seq = inner.next_seq;
            inner.next_seq += 1;
            inner.events.push(SessionEventRecord {
                seq,
                event,
                created_at_ms: now_ms(),
            });
        }
        Arc::new(Self {
            inner: AsyncMutex::new(inner),
        })
    }

    async fn snapshot(&self) -> Vec<SessionEventRecord> {
        self.inner.lock().await.events.clone()
    }
}

#[async_trait]
impl SessionService for MockSession {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        let head_seq = self.inner.lock().await.next_seq.saturating_sub(1);
        Ok(SessionHandle { id, head_seq })
    }
    async fn get_events(
        &self,
        _id: &SessionId,
        _from: Option<EventSeq>,
        _to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        Ok(self.inner.lock().await.events.clone())
    }
    async fn emit_event(
        &self,
        _id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError> {
        let mut inner = self.inner.lock().await;
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.events.push(SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        });
        Ok(seq)
    }
    async fn subscribe(
        &self,
        _id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        let (_tx, rx) = broadcast::channel(1);
        Ok(rx)
    }
    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        self.attach(id.clone()).await
    }
    async fn detach(&self, _id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }
}

struct NoopTools;

#[async_trait]
impl ToolService for NoopTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        Err(ToolError::NotFound {
            name: name.to_string(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
        None
    }
}

/// Text-only provider that counts calls so tests can assert whether the
/// Think phase actually invoked the LLM.
struct CountingProvider {
    calls: AtomicUsize,
    text: String,
}

impl CountingProvider {
    fn new(text: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            text: text.to_string(),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for CountingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let text = self.text.clone();
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse::text_only(text))
        })
    }
    fn name(&self) -> &str {
        "counting"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that always fails. `ContextCompactor` falls back to deterministic
/// truncation on provider error, which is exactly what we want the wiring
/// test to observe without standing up a happy-path summarization mock.
struct FailingProvider;

impl AiProvider for FailingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Err(crate::error::AlephError::provider("stub fail")) })
    }
    fn name(&self) -> &str {
        "failing"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

// -- Fixtures ----------------------------------------------------------------

fn sample_session_id() -> SessionId {
    SessionId::main("task10-wiring")
}

fn noop_sandbox_output() -> SandboxOutput {
    SandboxOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: Some(0),
        signal: None,
        truncated: false,
        duration_ms: 0,
    }
}

fn turn_started_event() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: uuid::Uuid::new_v4(),
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    }
}

fn user_message_event(text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
        },
        at: now_ms(),
    }
}

fn tiny_budget_config(budget: u64, warn: f64, critical: f64) -> ContextBudgetConfig {
    ContextBudgetConfig {
        token_budget: budget,
        warning_threshold: warn,
        critical_threshold: critical,
        // 1 char ≈ 1 token so tests get deterministic ratios from message length.
        token_estimate_ratio: 1.0,
        fresh_tail_count: 2,
        circuit_breaker_max: 10,
        diminishing_window: 16,
        diminishing_threshold: 1,
    }
}

// =============================================================================
// Test 1 — Budget FinalReply trips hit_limit and skips the LLM call
// =============================================================================
#[tokio::test]
async fn budget_final_reply_short_circuits_to_done_with_hit_limit() {
    // 100-char user message, budget=10, critical=0.50 → ratio ~= 10 → Critical.
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("should not fire");

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        stop_hooks: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None,
        power: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed on FinalReply");

    assert_eq!(
        state,
        TurnState::Done,
        "FinalReply directive must produce TurnState::Done"
    );
    assert!(
        harness.hit_limit(),
        "hit_limit must be set when the budget trips FinalReply"
    );
    assert_eq!(
        provider.call_count(),
        0,
        "LLM must NOT be called once the budget has forced FinalReply"
    );
}

// =============================================================================
// Test 2 — CompactAndContinue invokes the attached compactor before the LLM
// =============================================================================
#[tokio::test]
async fn budget_warning_invokes_compactor_before_llm() {
    // 80-char user message, budget=100, warn=0.50, critical=0.90 → warning zone.
    let user_text = "y".repeat(80);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("assistant reply");

    let budget = ContextBudget::new(&tiny_budget_config(100, 0.50, 0.90));
    // FailingProvider drives the compactor down its deterministic-truncation
    // fallback path. Reaching that branch at all proves `compact()` was invoked.
    let compactor = Arc::new(ContextCompactor::new(
        Arc::new(FailingProvider) as Arc<dyn AiProvider>,
        CompactorConfig {
            fresh_tail: 1,
            ..Default::default()
        },
    ));

    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        stop_hooks: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor.clone()),
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None,
        power: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed on CompactAndContinue");

    // A text-only LLM reply closes out as Done.
    assert_eq!(state, TurnState::Done);
    assert!(!harness.hit_limit(), "warning zone must not set hit_limit");
    assert_eq!(
        provider.call_count(),
        1,
        "LLM must be invoked exactly once after compaction"
    );

    // AssistantMessage was emitted, proving we walked the full turn.
    let events = session.snapshot().await;
    let assistant_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(assistant_count, 1);
}

// =============================================================================
// Test 3 — Stop-hook veto forces Continue and injects the veto as a user turn
// =============================================================================

struct AlwaysBlockHook {
    reason: String,
}

#[async_trait]
impl StopHookHandler for AlwaysBlockHook {
    fn name(&self) -> &str {
        "always-block"
    }
    async fn evaluate(
        &self,
        _ctx: &StopHookContext,
        _cancel: &CancellationToken,
    ) -> StopHookVerdict {
        StopHookVerdict::Block {
            reason: self.reason.clone(),
        }
    }
}

#[tokio::test]
async fn stop_hook_veto_forces_continue_and_injects_block_reason() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("done?")]);
    let provider = CountingProvider::new("all done");

    let hooks: Arc<Vec<Arc<dyn StopHookHandler>>> = Arc::new(vec![Arc::new(AlwaysBlockHook {
        reason: "tests not passing".to_string(),
    })]);

    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        stop_hooks: Some(hooks),
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None,
        power: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");

    assert_eq!(
        state,
        TurnState::Continue,
        "veto must flip Done → Continue so the model keeps running"
    );

    // The veto was persisted as a UserMessage so the next Think pass sees it.
    let events = session.snapshot().await;
    let veto_injected = events.iter().any(|r| match &r.event {
        SessionEvent::UserMessage { content, .. } => {
            content.text.contains("stop-hook veto") && content.text.contains("tests not passing")
        }
        _ => false,
    });
    assert!(
        veto_injected,
        "stop-hook block reason must be re-injected as a UserMessage; got events: {:#?}",
        events
    );
}

//! Tests for the reactive-compaction rescue path (Phase A — see
//! `harness::agent::think::try_reactive_compact_and_retry`).
//!
//! Mirrors claude-code's query.ts:1092 single-shot reactive compaction
//! pattern. The wire was previously dead: `RetryVerdict::CompactAndRetry`
//! was produced by `providers::llm_retry::classify` but no harness path
//! consumed it (the failover layer says "the harness context-compactor
//! owns this recovery path"). These tests pin the new behaviour so
//! future churn cannot silently regress the connection.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};

use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::error::{AlephError, Result as AlephResult};
use crate::harness::{AgentHarness, Harness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::orchestrator::dispatch::TerminateReason;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::sandbox::SandboxOutput;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::sync_primitives::{AtomicUsize, Ordering};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

// ---------------------------------------------------------------------------
// Local fixtures (kept independent of `think.rs` so refactors to that file
// can't accidentally remove tests' scaffolding).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockSessionInner {
    events: Vec<SessionEventRecord>,
    next_seq: EventSeq,
}

struct MockSession {
    inner: Mutex<MockSessionInner>,
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
            inner: Mutex::new(inner),
        })
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

struct EmptyTools;

#[async_trait]
impl ToolService for EmptyTools {
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
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

/// Provider that returns `prompt_too_long` on the first call, then a
/// happy-path text response. Used to verify the rescue path retries the
/// LLM call exactly once and surfaces a clean completion.
struct RecoverableOverflowProvider {
    calls: AtomicUsize,
    success_text: String,
}

impl RecoverableOverflowProvider {
    fn new(success_text: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            success_text: success_text.to_string(),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for RecoverableOverflowProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let success = self.success_text.clone();
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // String shape that `providers::llm_retry::classify` recognises
                // as `CompactAndRetry` (matches Anthropic's wire text).
                Err(AlephError::provider(
                    "prompt is too long: 250000 tokens > 200000 maximum",
                ))
            } else {
                Ok(ProviderResponse::text_only(success))
            }
        })
    }
    fn name(&self) -> &str {
        "recoverable_overflow"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that ALWAYS returns `prompt_too_long`. Used to verify the
/// rescue cap holds at `MAX_REACTIVE_COMPACT_ATTEMPTS = 1`.
struct PersistentOverflowProvider {
    calls: AtomicUsize,
}

impl PersistentOverflowProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for PersistentOverflowProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(AlephError::provider(
                "prompt is too long: 250000 tokens > 200000 maximum",
            ))
        })
    }
    fn name(&self) -> &str {
        "persistent_overflow"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that returns a non-overflow error. Verifies that the rescue
/// helper is a pure pass-through for any verdict that isn't
/// `CompactAndRetry`.
struct UnrelatedFailureProvider {
    calls: AtomicUsize,
}

impl UnrelatedFailureProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for UnrelatedFailureProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(AlephError::provider("400 bad request"))
        })
    }
    fn name(&self) -> &str {
        "unrelated_failure"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Stub compactor backend. The real `ContextCompactor` falls back to
/// deterministic truncation when its LLM call errors, so a failing
/// provider is enough to exercise the compactor path without standing up
/// a happy-path summariser. Mirrors `task10_wiring::FailingProvider`.
struct StubCompactorProvider;

impl AiProvider for StubCompactorProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { Err(AlephError::provider("stub summariser")) })
    }
    fn name(&self) -> &str {
        "stub_compactor"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_session_id() -> SessionId {
    SessionId::main("test")
}

fn noop_sandbox_output() -> SandboxOutput {
    SandboxOutput {
        exit_code: Some(0),
        ..Default::default()
    }
}

fn user_message_event(text: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        synthetic: false,
    }
}

fn turn_started_event() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: uuid::Uuid::new_v4(),
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    }
}

fn build_deps(
    session: Arc<MockSession>,
    llm: Arc<dyn AiProvider>,
    context_compactor: Option<Arc<ContextCompactor>>,
) -> HarnessDeps {
    HarnessDeps {
        session,
        tools: Arc::new(EmptyTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    }
}

fn stub_compactor() -> Arc<ContextCompactor> {
    Arc::new(ContextCompactor::new(
        Arc::new(StubCompactorProvider),
        CompactorConfig::default(),
    ))
}

/// A permissive budget so the reactive floor actually runs, while the large
/// window + high thresholds keep the proactive `before_turn` path at `Continue`
/// (it must not interfere with the reactive path under test).
fn budget_config() -> ContextBudgetConfig {
    ContextBudgetConfig {
        token_budget: 200_000,
        warning_threshold: 0.70,
        critical_threshold: 0.85,
        token_estimate_ratio: 4.0,
        fresh_tail_count: 2,
        circuit_breaker_max: 10,
        diminishing_window: 16,
        diminishing_threshold: 1,
        max_splits: 3,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: provider returns `prompt_too_long` once, then succeeds.
/// The harness consults the classifier, runs the compactor, and retries
/// once. The user sees a clean completion. The rescue counter advances
/// from 0 to 1.
#[tokio::test]
async fn rescue_succeeds_when_compactor_wired_and_retry_returns_clean() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("oversized input"),
    ]);
    let llm = RecoverableOverflowProvider::new("rescued response");
    let deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("rescue should turn the overflow into a clean turn");

    assert_eq!(state, TurnState::Done);
    assert_eq!(
        llm.call_count(),
        2,
        "provider should be called twice: initial overflow + post-compaction retry",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        1,
        "exactly one rescue slot should have been consumed",
    );
    // Terminate reason stays Completed on the happy path — the rescue is
    // observable only via the trace event + counter.
    assert_eq!(harness.terminate_reason(), TerminateReason::Completed);
}

/// Cap path: provider always returns `prompt_too_long`. The first rescue
/// succeeds in mutating messages but the retry still overflows, so the
/// helper surfaces `HarnessError::Llm` and stamps the
/// `ReactiveCompactExhausted` terminate reason. Subsequent provider
/// retries (e.g. inside the empty-response loop) are NOT issued because
/// the LLM error propagates immediately. The compactor cap guarantees
/// the helper cannot loop forever.
#[tokio::test]
async fn rescue_exhausts_when_retry_still_overflows() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("permanently oversized input"),
    ]);
    let llm = PersistentOverflowProvider::new();
    let deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    let harness = AgentHarness::new(deps);

    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert!(
        result.is_err(),
        "persistent overflow should surface as error"
    );
    // Initial call + one retry after compaction — the cap allows exactly
    // one rescue attempt.
    assert_eq!(llm.call_count(), 2);
    assert_eq!(harness.reactive_compact_attempts_for_tests(), 1);
    assert_eq!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
    );
}

/// No LLM compactor wired: a `prompt_too_long` error no longer hard-stops
/// immediately. The reactive fallback floors to fit (a no-op here — no budget
/// wired) and retries the provider ONCE; the retry still overflows, so the
/// helper surfaces `HarnessError::Llm` with `ReactiveCompactExhausted`. The
/// never-break contract: always attempt recovery before giving up.
#[tokio::test]
async fn overflow_floor_retries_then_propagates_when_no_compactor() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("oversized")]);
    let llm = PersistentOverflowProvider::new();
    let deps = build_deps(session.clone(), llm.clone(), None);
    let harness = AgentHarness::new(deps);

    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert!(result.is_err());
    assert_eq!(
        llm.call_count(),
        2,
        "no compactor → floor (no-op) + exactly one retry before propagating",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        0,
        "the no-compactor exit does not consume an LLM-compaction rescue slot",
    );
    assert_eq!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
    );
}

/// Never-break recovery: an overflow with no compactor wired but a budget
/// present floors the prompt to fit and retries ONCE; the retry succeeds, so the
/// run completes cleanly and does NOT stamp `ReactiveCompactExhausted`. This is
/// the core Task-5 behaviour: a full context window recovers instead of ending
/// the run.
#[tokio::test]
async fn overflow_floor_retry_recovers_to_clean_completion() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("oversized input"),
    ]);
    let llm = RecoverableOverflowProvider::new("recovered after floor");
    let mut deps = build_deps(session.clone(), llm.clone(), None);
    // Wire a budget so the deterministic floor in `compact_to_fit` actually runs.
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(&budget_config()))));
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("floor + retry must turn the overflow into a clean turn");

    assert_eq!(state, TurnState::Done);
    assert_eq!(
        llm.call_count(),
        2,
        "provider called twice: initial overflow + post-floor retry",
    );
    assert_ne!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
        "a recovered overflow must NOT hard-stop as ReactiveCompactExhausted",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        0,
        "the no-compactor floor path does not consume an LLM-compaction rescue slot",
    );
}

/// Unrelated provider errors (anything classifier does not tag as
/// `CompactAndRetry`) must bypass the rescue helper entirely: no
/// compactor invocation, no counter advance, no terminate-reason
/// stamping. The error propagates as `HarnessError::Llm` exactly as
/// before this Phase A wiring existed.
#[tokio::test]
async fn non_overflow_errors_bypass_rescue() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hi")]);
    let llm = UnrelatedFailureProvider::new();
    let deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    let harness = AgentHarness::new(deps);

    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert!(result.is_err());
    assert_eq!(llm.call_count(), 1, "no retry for non-overflow errors");
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        0,
        "non-CompactAndRetry verdict must NOT consume a rescue slot",
    );
    // Terminate reason stays at default `Completed` — the rescue helper
    // doesn't touch it on the pass-through path.
    assert_eq!(harness.terminate_reason(), TerminateReason::Completed);
}

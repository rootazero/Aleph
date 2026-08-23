//! Tests for the reactive-compaction rescue path (Phase A — see
//! `context::compact::rescue::try_reactive_compact_and_retry`, reached from the
//! loop through `RescueHost`/`RescueCx`; these tests drive the whole turn, so
//! they pin the seam end-to-end, not the algorithm in isolation).
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
use crate::context::compact::rescue::MAX_REACTIVE_COMPACT_ATTEMPTS;
use crate::error::{AlephError, Result as AlephResult};
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::orchestrator::dispatch::TerminateReason;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
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
        // Match the real store: seqs are assigned from 1 (0 = empty head).
        let mut inner = MockSessionInner {
            next_seq: 1,
            ..MockSessionInner::default()
        };
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
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        // Honor the seq range like the real store (`seq >= from && seq <= to`)
        // so range-based production reads (watermark tails) stay testable.
        let from = from.unwrap_or(0);
        let to = to.unwrap_or(EventSeq::MAX);
        Ok(self
            .inner
            .lock()
            .await
            .events
            .iter()
            .filter(|r| r.seq >= from && r.seq <= to)
            .cloned()
            .collect())
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
/// rescue cap holds at the value defined in
/// `MAX_REACTIVE_COMPACT_ATTEMPTS` (currently 2).
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

/// Returns `prompt_too_long` on the first two calls, then clean text. Drives
/// exit #4's I1 fallback: the post-compaction retry (#2) still overflows, so it
/// is the deterministic-floor retry (#3) that recovers.
struct OverflowTwiceThenTextProvider {
    calls: AtomicUsize,
    success_text: String,
}

impl OverflowTwiceThenTextProvider {
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

impl AiProvider for OverflowTwiceThenTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let success = self.success_text.clone();
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(AlephError::provider(
                    "prompt is too long: 250000 tokens > 200000 maximum",
                ))
            } else {
                Ok(ProviderResponse::text_only(success))
            }
        })
    }
    fn name(&self) -> &str {
        "overflow_twice_then_text"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Errors with `prompt_too_long` on the first call, then returns a BILLED
/// response that is *still* `ContextWindowExceeded`. Drives
/// `reactive_fit_and_retry`'s still-overflow discard: that response never
/// becomes an `AssistantMessage`, but the provider billed it (Anthropic carries
/// usage in the same `message_delta` frame as the stop reason), so the harness
/// must fold its tokens into the run totals before dropping it.
struct BilledStillOverflowProvider {
    calls: AtomicUsize,
}

impl BilledStillOverflowProvider {
    /// Tokens the still-overflow response reports: 900 + 100 = 1000 billed.
    const BILLED_TOKENS: u64 = 1_000;

    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for BilledStillOverflowProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(AlephError::provider(
                    "prompt is too long: 250000 tokens > 200000 maximum",
                ));
            }
            Ok(ProviderResponse {
                stop_reason: crate::providers::adapter::StopReason::ContextWindowExceeded,
                usage: Some(crate::providers::adapter::TokenUsage {
                    input_tokens: 900,
                    output_tokens: 100,
                    ..Default::default()
                }),
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "billed_still_overflow"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that always returns clean text (no overflow). The seeded history is
/// compacted proactively *before* this call, so the call itself succeeds — the
/// whole point of the never-break regression: a reloaded near-full session must
/// reach a normal completion, not a hard-stop.
struct PlainTextProvider {
    calls: AtomicUsize,
    text: String,
}

impl PlainTextProvider {
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

impl AiProvider for PlainTextProvider {
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
        "plain_text"
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
        author_user_id: None,
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
        llm,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
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
        summarizer_input_budget: 48_000,
        circuit_breaker_max: 10,
        max_splits: 3,
    }
}

/// A near-full budget: `token_budget` small and 1 char ≈ 1 token, so the seeded
/// history is critically over budget on turn 0 — exactly what reloading a
/// near-full conversation looks like to `before_turn`.
fn near_full_budget_config() -> ContextBudgetConfig {
    ContextBudgetConfig {
        token_budget: 100,
        warning_threshold: 0.40,
        critical_threshold: 0.85,
        token_estimate_ratio: 1.0,
        fresh_tail_count: 2,
        summarizer_input_budget: 48_000,
        circuit_breaker_max: 10,
        max_splits: 3,
    }
}

fn assistant_message_event(text: &str) -> SessionEvent {
    SessionEvent::AssistantMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        usage: None,
        at: now_ms(),
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

/// Exhaustion path: provider ALWAYS returns `prompt_too_long`. Flow: primary
/// call (#1) → LLM compact → retry (#2, still overflow error) → exit #4's I1
/// fallback floors deterministically and retries once more (#3, still overflow)
/// → surfaces `HarnessError::Llm` + `ReactiveCompactExhausted`. Bounded: floor +
/// a single extra retry, then honest surface — the helper cannot loop forever
/// even when nothing can shrink the prompt enough.
/// The cap lives in the Context layer (it is the rescue *policy*); the slot
/// lives in the harness (it is per-run *state*). The seam only means something
/// if the slot actually reads the cap — a `compare_exchange(0, 1)` hardcoding
/// the same number would pass every behavioural test above while making
/// `MAX_REACTIVE_COMPACT_ATTEMPTS` decorative: raise it and nothing happens.
#[test]
fn the_rescue_slot_is_bounded_by_the_context_layers_cap_not_a_hardcoded_one() {
    // The provider is never called — this exercises the slot, not a turn.
    let deps = build_deps(
        MockSession::new(vec![]),
        PlainTextProvider::new("unused"),
        Some(stub_compactor()),
    );
    let harness = AgentHarness::new(deps);

    for reserved in 0..MAX_REACTIVE_COMPACT_ATTEMPTS {
        assert!(
            harness.try_reserve_reactive_compact(),
            "slot {reserved} of {MAX_REACTIVE_COMPACT_ATTEMPTS} must be claimable",
        );
    }
    assert!(
        !harness.try_reserve_reactive_compact(),
        "the run's LLM-compaction budget is spent; the caller must fall back to the floor",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        MAX_REACTIVE_COMPACT_ATTEMPTS,
    );
}

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
    // #1 primary + #2 post-compact retry + #3 post-floor retry (I1) = 3 calls,
    // then honest surface. The rescue slot is still reserved exactly once (the
    // deterministic floor fallback consumes no additional LLM-compaction slot).
    assert_eq!(llm.call_count(), 3);
    assert_eq!(harness.reactive_compact_attempts_for_tests(), 1);
    assert_eq!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
    );
}

/// I1 recovery: the post-compaction retry ALSO overflows (error-style, as
/// OpenAI-compatible proxies report it), so exit #4 no longer hard-stops — it
/// floors deterministically and retries once more, which recovers. Calls: #1
/// primary + #2 post-compact retry (overflow) + #3 post-floor retry (clean).
#[tokio::test]
async fn exit4_still_overflow_error_floors_and_recovers() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("oversized input that still overflows after the summary"),
    ]);
    let llm = OverflowTwiceThenTextProvider::new("recovered after the floor");
    let deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("exit #4 must floor + retry, not hard-stop, when the retry still overflows");

    assert_eq!(state, TurnState::Done);
    assert_eq!(
        llm.call_count(),
        3,
        "primary + post-compact retry (overflow) + post-floor retry (I1) = 3 calls",
    );
    assert_ne!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
        "a recovered exit-#4 overflow must NOT hard-stop",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        1,
        "the deterministic-floor fallback consumes no additional rescue slot",
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

/// The still-overflow response `reactive_fit_and_retry` discards is a real
/// billed round-trip and must be accounted before it is dropped — every other
/// discard point in the harness (empty-response loop, max_output_tokens loop,
/// overflow drain, grace turn) already does. It never becomes an
/// `AssistantMessage`, so `total_tokens()` / `token_breakdown()` (→ FlowOutcome)
/// are the only places those tokens can ever surface.
#[tokio::test]
async fn still_overflow_response_is_accounted_before_being_discarded() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("oversized")]);
    let llm = BilledStillOverflowProvider::new();
    // No compactor + a budget → the overflow error routes straight to
    // `reactive_fit_and_retry`, whose retry returns the billed still-overflow.
    let mut deps = build_deps(session.clone(), llm.clone(), None);
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(&budget_config()))));
    let harness = AgentHarness::new(deps);

    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert!(
        result.is_err(),
        "a prompt that still overflows after the floor must surface honestly",
    );
    assert_eq!(
        llm.call_count(),
        2,
        "primary overflow error + one post-floor retry",
    );
    assert_eq!(
        harness.terminate_reason(),
        TerminateReason::ReactiveCompactExhausted,
    );
    assert_eq!(
        harness.total_tokens(),
        BilledStillOverflowProvider::BILLED_TOKENS,
        "the discarded still-overflow response was billed; its tokens must be in the run total",
    );
    assert_eq!(
        harness.token_breakdown().total(),
        BilledStillOverflowProvider::BILLED_TOKENS,
        "the per-component breakdown must agree with total_tokens()",
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

// ---------------------------------------------------------------------------
// End-to-end never-break regression (the user's original bug). Distinct from
// the reactive rescue tests above: this exercises the PROACTIVE path — a
// reloaded near-full session that is critical on turn 0 must compact-to-fit and
// continue, never hard-stopping on ContextBudgetExhausted / ReactiveCompactExhausted.
// ---------------------------------------------------------------------------

/// Reloading a near-full conversation and typing one more message must CONTINUE
/// to a normal answer, not brick the session. A large seeded history + a small
/// budget makes turn 0 critically over budget → `before_turn` returns
/// `CompactToFit` → the harness floors to fit and falls through to the LLM,
/// which answers cleanly. Neither the proactive (`ContextBudgetExhausted`) nor
/// the reactive (`ReactiveCompactExhausted`) hard-stop may fire.
#[tokio::test]
async fn reloaded_near_full_session_continues_not_bricked() {
    // Seed a realistic near-full history: alternating user/assistant turns whose
    // combined length (~850 chars ≈ 850 tokens at ratio 1.0) dwarfs the 100-token
    // budget, so turn 0 is critical.
    let mut events = vec![turn_started_event()];
    for i in 0..6 {
        events.push(user_message_event(&format!(
            "earlier user turn {i}: a chunk of prior conversation text padding the window"
        )));
        events.push(assistant_message_event(&format!(
            "earlier assistant turn {i}: a chunk of prior reply text padding the window"
        )));
    }
    // The freshly-typed message on the reloaded session.
    events.push(user_message_event("open the html"));
    let session = MockSession::new(events);

    let llm = PlainTextProvider::new("here is the opened html");
    let mut deps = build_deps(session.clone(), llm.clone(), None);
    // Budget wired so turn 0 is critical and the proactive floor runs.
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(
        &near_full_budget_config(),
    ))));
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("a reloaded near-full session must continue, not error out");

    assert_eq!(state, TurnState::Done);
    // Never-break: neither hard-stop reason may be stamped.
    let reason = harness.terminate_reason();
    assert_ne!(
        reason,
        TerminateReason::ContextBudgetExhausted,
        "a full context window must not hard-stop the run (proactive P1)",
    );
    assert_ne!(
        reason,
        TerminateReason::ReactiveCompactExhausted,
        "a full context window must not hard-stop the run (reactive P2)",
    );
    assert!(
        llm.call_count() >= 1,
        "the harness must compact and still issue the LLM request",
    );
    // The user gets a real, non-empty answer.
    let recorded = session
        .get_events(&sample_session_id(), None, None)
        .await
        .expect("get_events");
    let answered = recorded.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. } if content.text == "here is the opened html")
    });
    assert!(
        answered,
        "the run must persist the model's non-empty final text; got: {recorded:#?}",
    );
}

// ---------------------------------------------------------------------------
// Silent truncated-overflow guard (pi `overflow.ts` Case 3 parity): a provider
// that reports stop_reason=length with ZERO output once the INPUT alone fills
// the window — no error, no ContextWindowExceeded. The guard must route this
// to reactive compaction instead of the empty-retry / resume-nudge loops.
// ---------------------------------------------------------------------------

/// Provider emulating the z.ai / MiMo silent-overflow shape: the first
/// `silent_calls` calls return `stop_reason == MaxTokens` with no content and
/// a usage report whose prompt alone is `prompt_tokens`; after that, clean
/// text. `usize::MAX` = never recovers on its own.
struct SilentTruncatedOverflowProvider {
    calls: AtomicUsize,
    prompt_tokens: u32,
    silent_calls: usize,
    success_text: String,
}

impl SilentTruncatedOverflowProvider {
    fn new(prompt_tokens: u32, silent_calls: usize, success_text: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            prompt_tokens,
            silent_calls,
            success_text: success_text.to_string(),
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for SilentTruncatedOverflowProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let success = self.success_text.clone();
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.silent_calls {
                Ok(ProviderResponse {
                    stop_reason: crate::providers::adapter::StopReason::MaxTokens,
                    usage: Some(crate::providers::adapter::TokenUsage {
                        input_tokens: self.prompt_tokens,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            } else {
                Ok(ProviderResponse::text_only(success))
            }
        })
    }
    fn name(&self) -> &str {
        "silent_truncated_overflow"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider returning `MaxTokens` WITH partial text and a full-window prompt:
/// the genuine output-cap shape 3b exists for. The guard must NOT fire on it.
struct GenuineOutputCapProvider {
    calls: AtomicUsize,
    prompt_tokens: u32,
}

impl GenuineOutputCapProvider {
    fn new(prompt_tokens: u32) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            prompt_tokens,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for GenuineOutputCapProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse {
                stop_reason: crate::providers::adapter::StopReason::MaxTokens,
                usage: Some(crate::providers::adapter::TokenUsage {
                    input_tokens: self.prompt_tokens,
                    output_tokens: 500,
                    ..Default::default()
                }),
                ..ProviderResponse::text_only("partial answer chunk".to_string())
            })
        })
    }
    fn name(&self) -> &str {
        "genuine_output_cap"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// The core fix: `MaxTokens` + zero output + prompt alone ≥ budget IS a
/// context overflow. The guard routes it to the reactive-compaction rescue —
/// the stub compactor's LLM call fails, so the deterministic floor runs and
/// the single retry recovers. Before the guard, this shape burned the
/// empty-retry and resume-nudge budgets and died as EmptyResponseExhausted
/// without ever compacting.
#[tokio::test]
async fn silent_truncated_overflow_routes_to_reactive_compaction() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("oversized input"),
    ]);
    // Prompt alone (250k) exceeds the 200k budget; recovers on the 2nd call.
    let llm = SilentTruncatedOverflowProvider::new(250_000, 1, "rescued from silent overflow");
    let mut deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(&budget_config()))));
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("the silent overflow must be rescued into a clean turn");

    assert_eq!(state, TurnState::Done);
    assert_eq!(
        llm.call_count(),
        2,
        "guard fires on the first response; the compactor's LLM fails, so the floor + one retry recovers",
    );
    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        1,
        "the rescue consumes exactly one LLM-compaction slot",
    );
    // The truncated first call billed the full 250k prompt; it was discarded
    // by the rescue, so its tokens must be in the run total.
    assert!(
        harness.total_tokens() >= 250_000,
        "the discarded silent-overflow call was billed; got {}",
        harness.total_tokens(),
    );
}

/// Negative control: the same MaxTokens + zero-output shape with a prompt
/// BELOW the budget is not a context overflow — the guard stays silent, no
/// rescue slot is consumed, and the legacy empty/nudge retry cadence runs
/// unchanged (1 primary + 2 empty retries + 3 resume-nudge retries = 6).
#[tokio::test]
async fn guard_stays_silent_when_prompt_is_below_budget() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hi")]);
    let llm = SilentTruncatedOverflowProvider::new(1_000, usize::MAX, "never");
    let mut deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(&budget_config()))));
    let harness = AgentHarness::new(deps);

    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        0,
        "a sub-budget prompt must NOT consume a rescue slot",
    );
    assert_eq!(
        llm.call_count(),
        6,
        "legacy cadence untouched: 1 primary + 2 empty retries + 3 resume nudges",
    );
}

/// Negative control: `MaxTokens` WITH partial text is the genuine output-cap
/// case — even with a full-window prompt, the resume-nudge loop (not
/// compaction) is the right recovery. The guard must not fire.
#[tokio::test]
async fn guard_stays_silent_for_genuine_output_cap_with_partial_text() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("write a very long essay"),
    ]);
    let llm = GenuineOutputCapProvider::new(250_000);
    let mut deps = build_deps(session.clone(), llm.clone(), Some(stub_compactor()));
    deps.context_budget = Some(Arc::new(Mutex::new(ContextBudget::new(&budget_config()))));
    let harness = AgentHarness::new(deps);

    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;

    assert_eq!(
        harness.reactive_compact_attempts_for_tests(),
        0,
        "partial text = genuine output cap; compaction must NOT fire",
    );
    assert_eq!(
        llm.call_count(),
        4,
        "1 primary + 3 resume-nudge retries, no empty retries (text is non-empty)",
    );
}

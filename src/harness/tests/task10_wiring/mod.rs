//! Task-10 (Phase 6b) wiring tests — the Task-6 triad on `HarnessDeps`.
//!
//! Covers three behaviours inherited from the retiring `AgentLoop`:
//!   1. **Budget — `FinalReply`** trips `hit_limit` and short-circuits to
//!      `TurnState::Done` without invoking the LLM.
//!   2. **Budget — `CompactAndContinue`** invokes the attached
//!      `ContextCompactor` before the LLM call.
//!   3. **Stop-hook veto** forces one additional `TurnState::Continue`
//!      even when the LLM produced no tool calls.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio_util::sync::CancellationToken;

use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::error::Result as AlephResult;
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};
use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};
use crate::verification::{StopHookVerifier, ToolLoopVerifier, VerifierChain};

// -- Minimal mocks (kept local; the think/act suites have larger copies) -----

mod extras;

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
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
        std::sync::Arc::from([])
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

fn turn_started_event() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: uuid::Uuid::new_v4(),
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    }
}

fn assistant_message_event_with_text(text: &str) -> SessionEvent {
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

fn tiny_budget_config(budget: u64, warn: f64, critical: f64) -> ContextBudgetConfig {
    ContextBudgetConfig {
        token_budget: budget,
        warning_threshold: warn,
        critical_threshold: critical,
        // 1 char ≈ 1 token so tests get deterministic ratios from message length.
        token_estimate_ratio: 1.0,
        fresh_tail_count: 2,
        circuit_breaker_max: 10,
        max_splits: 3,
    }
}

// =============================================================================
// Test 1 — Critical context pressure with a prior assistant text: the harness
// compacts to fit and continues into the normal LLM call. It never hard-stops.
// =============================================================================
#[tokio::test]
async fn budget_critical_compacts_and_continues_with_prior_text() {
    // 100-char user message, budget=10, critical=0.50 → ratio ~= 10 → Critical.
    // Includes a prior assistant text — compact-then-continue fires, LLM IS called.
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event(&user_text),
        assistant_message_event_with_text("here is your answer"),
    ]);
    let provider = CountingProvider::new("continued after compaction");

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn must succeed: critical pressure compacts and continues");

    // Never-break: critical context pressure compacts in place, then the normal
    // LLM call fires. The run must NOT hard-stop (no hit_limit, no
    // ContextBudgetExhausted) even though a prior assistant text exists on the log.
    assert_eq!(
        state,
        TurnState::Done,
        "compact-then-continue ends the turn via a normal LLM completion"
    );
    assert!(
        !harness.hit_limit(),
        "critical pressure must compact and continue, never set hit_limit",
    );
    assert_ne!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::ContextBudgetExhausted,
        "context fill must never terminate the run",
    );
    assert_eq!(
        provider.call_count(),
        1,
        "the LLM must be called exactly once after compaction (not skipped as in the old FinalReply path)",
    );
}

// =============================================================================
// Test 1b — Critical context pressure with no prior assistant text: compact to
// fit, then the normal LLM call produces the terminal text. Never a hard-stop.
// =============================================================================
#[tokio::test]
async fn budget_critical_compacts_and_continues_no_prior_text() {
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("continued after compaction");

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn must succeed: critical pressure compacts and continues");

    assert_eq!(state, TurnState::Done);
    assert!(
        !harness.hit_limit(),
        "critical pressure must compact and continue, never set hit_limit",
    );
    assert_eq!(
        provider.call_count(),
        1,
        "the LLM must be called exactly once after compaction",
    );

    // The LLM's response after compaction must reach the session log (the user
    // gets a real answer, not a hard-stop).
    let events = session.snapshot().await;
    let text_present = events.iter().any(|r| match &r.event {
        SessionEvent::AssistantMessage { content, .. } => {
            content.text == "continued after compaction"
        }
        _ => false,
    });
    assert!(
        text_present,
        "the post-compaction LLM response must be persisted as an AssistantMessage; got: {:#?}",
        events
    );
}

// Test 1c (tool_use-only prior assistant message) covered as a focused unit
// test on `last_assistant_has_text` in `src/harness/agent/think.rs` — driving
// it as an integration test depends on the prompt-builder's handling of an
// unmatched `tool_use`, which is not the behavior under test here.

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
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor.clone()),
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
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
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(StopHookVerifier::new(hooks)))
            .build(),
    );

    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
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
            content.text.contains("verifier veto") && content.text.contains("tests not passing")
        }
        _ => false,
    });
    assert!(
        veto_injected,
        "verifier block reason must be re-injected as a UserMessage; got events: {:#?}",
        events
    );
}

// =============================================================================
// Test 4 — Stage 6a (#10) ToolLoopVerifier vetoes repeated tool_call end-to-end
// =============================================================================

/// Provider that always emits the *same* tool_call with empty text. Drives the
/// harness into the precise pathology ToolLoopVerifier is designed to catch:
/// pure repetition with no thinking text.
struct RepeatingToolCallProvider {
    calls: AtomicUsize,
}

impl RepeatingToolCallProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
}

impl AiProvider for RepeatingToolCallProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![crate::providers::adapter::NativeToolCall {
                    thought_signature: None,
                    id: "loop-id".to_string(),
                    name: "loop_tool".to_string(),
                    arguments: serde_json::json!({"x": 1}),
                }],
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "repeat"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn tool_loop_verifier_vetoes_repeated_tool_call_with_no_text() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("loop on me")]);
    let provider = RepeatingToolCallProvider::new();
    // Threshold = 5; max_iterations cap small enough that the loop ends
    // shortly after the verifier should have fired but before the
    // per-model steer_max cap (conservative default = 10).
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(ToolLoopVerifier::new()))
            .build(),
    );
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: Some(7),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = crate::harness::NoopHarnessCallback;
    harness
        .run(&sample_session_id(), &mut cb, &cancel)
        .await
        .expect("harness.run should complete via hit_limit");

    // After 7 iterations of the same tool_call with no text, ToolLoopVerifier
    // must have fired at least once and injected `[verifier veto] ...` into
    // the session as a UserMessage.
    let events = session.snapshot().await;
    let veto_count = events
        .iter()
        .filter(|r| {
            matches!(&r.event,
                SessionEvent::UserMessage { content, .. }
                if content.text.starts_with("[verifier veto]"))
        })
        .count();
    assert!(
        veto_count >= 1,
        "expected ≥1 [verifier veto] UserMessage; got {} events: {:#?}",
        events.len(),
        events
    );
    assert!(
        harness.hit_limit(),
        "max_iterations=7 should trip hit_limit",
    );
}

// =============================================================================
// Test — ToolLoopVerifier Halt is salvaged, not cold-terminated. The provider
// loops an identical tool call until Tier-1 halts (8×), then — when it sees the
// salvage nudge appended by the grace turn — returns a final deliverable. This
// covers ① (salvage grace turn on halt) and ② (orphan tool_use closed so the
// salvage prompt keeps the looped turns' context).
// =============================================================================

/// Loops an identical tool call (so `ToolLoopVerifier` Tier-1 halts), but emits
/// final text once the grace turn's salvage nudge appears in the prompt.
struct LoopThenSalvageProvider {
    calls: AtomicUsize,
}

impl LoopThenSalvageProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
}

impl AiProvider for LoopThenSalvageProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        // The grace turn appends GRACE_NUDGE_TOOL_LOOP_HALT, which contains this
        // phrase, as the trailing user message. Detect it to switch to salvage.
        let salvage = payload
            .messages
            .last()
            .map(|m| {
                m.text_content()
                    .contains("produce your best final deliverable")
            })
            .unwrap_or(false);
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if salvage {
                return Ok(ProviderResponse {
                    text: Some("Here is the report built from what I gathered.".to_string()),
                    ..Default::default()
                });
            }
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![crate::providers::adapter::NativeToolCall {
                    thought_signature: None,
                    id: "loop-id".to_string(),
                    name: "loop_tool".to_string(),
                    arguments: serde_json::json!({"x": 1}),
                }],
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "loop_then_salvage"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn tool_loop_halt_fires_salvage_grace_turn_and_closes_orphan() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("loop on me")]);
    let provider = LoopThenSalvageProvider::new();
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(ToolLoopVerifier::new()))
            .build(),
    );
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        // High enough that the Tier-1 halt (at 8 identical calls) fires before
        // the iteration cap, so we exercise the halt path, not max_iterations.
        max_iterations: Some(20),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = NoopHarnessCallback;
    harness
        .run(&sample_session_id(), &mut cb, &cancel)
        .await
        .expect("harness.run should complete via halt");

    // Terminated via the loop halt, not the iteration cap.
    assert!(
        matches!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::StopHookHalt { .. }
        ),
        "expected StopHookHalt, got {:?}",
        harness.terminate_reason()
    );

    let events = session.snapshot().await;

    // ① Salvage: a grace AssistantMessage with the deliverable text exists.
    let salvaged = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. }
            if content.text.contains("built from what I gathered"))
    });
    assert!(
        salvaged,
        "expected a salvage AssistantMessage from the grace turn; events: {events:#?}"
    );

    // ② Orphan closed: the looped tool_use got a synthetic ToolError so the
    // prompt builder would not drop it.
    let closed_orphan = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::ToolError { call_id, error, .. }
            if call_id == "loop-id" && error.contains("not executed"))
    });
    assert!(
        closed_orphan,
        "expected a ToolError closing the orphaned 'loop-id' tool_use; events: {events:#?}"
    );
}

// =============================================================================
// Test — A tool that overruns its own wall-clock budget comes back as a
// recoverable `ToolError::Timeout`, not a run abort. The budget belongs to the
// tool layer now (`ScopedToolService::execute_inner`, below the approval gate),
// so this drives the real production `ToolService` rather than a double that
// merely *advertises* a budget through `describe()` — the harness no longer
// reads it, precisely so an operator's approval time cannot be charged to the
// tool's clock. Cycle 3.
// =============================================================================

/// A `LoopTool` declaring a 50ms budget whose `execute()` sleeps 200ms.
struct SleepyBudgetedTool;

#[async_trait]
impl crate::tools::runtime::LoopTool for SleepyBudgetedTool {
    fn name(&self) -> &str {
        "sleepy_tool"
    }
    fn description(&self) -> &str {
        "sleeps past its own budget"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _cancel: CancellationToken,
    ) -> crate::tools::runtime::ToolResult {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        crate::tools::runtime::ToolResult::Success {
            output: serde_json::json!({"ok": true}),
        }
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(50)
    }
}

fn sleepy_tool_service() -> Arc<dyn ToolService> {
    let mut registry = crate::tools::runtime::LoopToolRegistry::new();
    registry.register(Box::new(SleepyBudgetedTool));
    Arc::new(crate::tools::ScopedToolService::new(
        Arc::new(registry),
        std::collections::BTreeSet::new(),
    ))
}

/// Provider that emits exactly one tool call for `sleepy_tool`.
struct OneShotSleepyCallProvider;

impl AiProvider for OneShotSleepyCallProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![crate::providers::adapter::NativeToolCall {
                    thought_signature: None,
                    id: "sleepy-id".to_string(),
                    name: "sleepy_tool".to_string(),
                    arguments: serde_json::json!({}),
                }],
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "sleepy"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn per_tool_budget_overrun_recovers_as_tool_error_not_run_abort() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("call the slow tool"),
    ]);
    let provider = Arc::new(OneShotSleepyCallProvider);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: sleepy_tool_service(),
        llm: provider as Arc<dyn AiProvider>,
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
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
        turn_timeout: Some(std::time::Duration::from_secs(60)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    let started = std::time::Instant::now();
    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;
    let elapsed = started.elapsed();

    // The tool's own 50ms budget fires well before the 60s `turn_timeout` — the
    // per-tool ceiling is what bounds the call, not the harness fallback.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "the tool's 50ms budget must fire well before the 60s turn_timeout; saw {elapsed:?}",
    );
    // ...and the overrun is RECOVERABLE: the turn completes instead of aborting
    // the whole run.
    assert!(
        result.is_ok(),
        "a budget overrun must NOT abort the run; got: {result:?}",
    );
    // Recorded as `ToolError::Timeout` — the variant, not merely timeout-flavoured
    // prose. It is the one thing `ToolError::is_retryable()` reads, and act.rs's
    // cross-batch memo now only bans non-retryable failures, so the retry this
    // error invites is actually permitted on the next batch.
    let events = session.snapshot().await;
    assert!(
        events.iter().any(|r| matches!(
            &r.event,
            crate::session::events::SessionEvent::ToolError { error, .. }
                if error.contains("timed out after")
        )),
        "the overrun must be recorded as a recoverable ToolError::Timeout",
    );
}

// =============================================================================
// Test — Veto cap follows per-model steer_max, not the old global const.
// With steer_max=2 the harness must fire a wrap-up grace turn after 2 vetoes,
// materially sooner than the old MAX_VERIFIER_VETOS=10 constant would.
// =============================================================================

#[tokio::test]
async fn veto_cap_follows_profile_steer_max() {
    // Build a profile with steer_max=2; all other fields stay conservative.
    // conservative() repeat_threshold=5, halt_threshold=8.
    // ToolLoopVerifier emits Veto (not Halt) for runs in [5,8) identical calls.
    // With steer_max=2, the harness must terminate (grace/HitLimit) after
    // exactly 2 accumulated vetoes — well before the old const-10 threshold.
    let profile = crate::verification::ModelRobustnessProfile {
        steer_max: 2,
        ..crate::verification::ModelRobustnessProfile::conservative()
    };

    let session = MockSession::new(vec![turn_started_event(), user_message_event("loop on me")]);
    let provider = RepeatingToolCallProvider::new();
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(ToolLoopVerifier::new()))
            .build(),
    );
    // max_iterations set high enough that the steer_max cap (2 vetoes) fires
    // long before the iteration ceiling.  With repeat_threshold=5, the first
    // Veto fires on turn 5; second on turn 6; then the cap triggers a grace turn.
    // 20 iterations is safely above 6 and safely below old-const-10's turn count.
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: profile,
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: Some(20),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = NoopHarnessCallback;
    harness
        .run(&sample_session_id(), &mut cb, &cancel)
        .await
        .expect("harness.run should complete via steer_max cap");

    // The run must terminate via the VerifierVeto cap, not the iteration cap.
    assert!(
        matches!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::VerifierVeto { .. }
        ),
        "expected VerifierVeto termination with steer_max=2; got {:?}",
        harness.terminate_reason()
    );
    assert!(
        harness.hit_limit(),
        "hit_limit must be set on VerifierVeto termination"
    );

    // The provider must have been called materially fewer times than the old
    // const-10 behavior would require.  With steer_max=2, vetoes fire on turns
    // 5 and 6 (first time run==repeat_threshold, second time run==6), so the
    // grace turn fires after turn 6 — well under 10+repeat_threshold=15.
    let calls = provider.calls.load(Ordering::SeqCst);
    assert!(
        calls < 15,
        "with steer_max=2, run must end well before old-const-10 turn count; got {calls} LLM calls",
    );
}

// =============================================================================
// Task-5 regression guard — grace-turn payload has no orphaned tool_use ids.
//
// Drives a Tier-1 Halt (8× identical tool_use → conservative halt_threshold)
// and inspects the *raw RequestPayload* the grace turn sends to the provider.
// Every `tool_use` id in any assistant message in the payload must have a
// matching `ToolResult` or `ToolError` message that follows it.  No orphan →
// no Anthropic HTTP 400.
//
// Resolution: `close_unexecuted_tool_uses` emits a synthetic `ToolError` for
// every id in `response.tool_calls` *before* `fire_boundary_grace_turn`
// re-fetches the session log and rebuilds the prompt via `build_prompt`.
// `build_prompt` scans forward from each `AssistantMessage` to build the
// `resolved` set (prompt.rs ~line 70-77) — a non-adjacency scan — so the
// `[stop hook halt]` UserMessage emitted *between* the assistant turn and the
// synthetic ToolErrors does not break pairing.  The test locks in this
// invariant as a compile-time regression guard.
// =============================================================================

/// Loops an identical tool_use until Tier-1 Halt fires, then answers the
/// grace turn with text. Captures every RequestPayload it receives so the
/// test can inspect the grace-turn payload for orphaned ids.
struct RecordingHaltProvider {
    calls: AtomicUsize,
    /// Clones of every payload's messages, captured before returning.
    /// Outer index = call number; inner = the message vector clone.
    recorded: tokio::sync::Mutex<Vec<Vec<UnifiedMessage>>>,
}

impl RecordingHaltProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            recorded: tokio::sync::Mutex::new(Vec::new()),
        })
    }
}

impl AiProvider for RecordingHaltProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        // Clone messages for later inspection before returning.
        let messages_clone: Vec<UnifiedMessage> = payload.messages.to_vec();
        // Detect the grace turn: the last message must be a User message
        // containing the salvage nudge.  `LoopThenSalvageProvider` above uses
        // the same sentinel text ("produce your best final deliverable").
        let is_grace = payload
            .messages
            .last()
            .map(|m| {
                m.text_content()
                    .contains("produce your best final deliverable")
            })
            .unwrap_or(false);
        Box::pin(async move {
            self.recorded.lock().await.push(messages_clone);
            self.calls.fetch_add(1, Ordering::SeqCst);
            if is_grace {
                return Ok(ProviderResponse::text_only(
                    "salvage answer from recording provider".to_string(),
                ));
            }
            // Non-grace: emit the same tool_use every time so the Tier-1 Halt
            // fires on the 8th call (conservative profile halt_threshold = 8).
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![crate::providers::adapter::NativeToolCall {
                    thought_signature: None,
                    id: "halt-id".to_string(),
                    name: "halt_tool".to_string(),
                    arguments: serde_json::json!({"n": 1}),
                }],
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "recording-halt"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn grace_turn_payload_has_no_orphaned_tool_calls() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("loop on me")]);
    let provider = RecordingHaltProvider::new();
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(ToolLoopVerifier::new()))
            .build(),
    );
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        // conservative: halt_threshold=8; provider loops 8× identical calls
        // then the grace turn fires.
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: Some(20),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = NoopHarnessCallback;
    harness
        .run(&sample_session_id(), &mut cb, &cancel)
        .await
        .expect("harness.run should complete via halt");

    // Terminated via the Tier-1 Halt (StopHookHalt), not the iteration cap.
    assert!(
        matches!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::StopHookHalt { .. }
        ),
        "expected StopHookHalt; got {:?}",
        harness.terminate_reason()
    );

    // The grace turn is the LAST payload the provider received.
    let recorded = provider.recorded.lock().await;
    assert!(
        !recorded.is_empty(),
        "provider must have been called at least once"
    );
    let grace_payload_messages = recorded.last().expect("at least one recorded payload");

    // Build the set of tool_use ids that appear in ANY assistant message
    // in the grace payload.
    let mut tool_use_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in grace_payload_messages.iter() {
        if let UnifiedMessage::Assistant { content } = msg {
            for block in content {
                if let crate::providers::message::ContentBlock::ToolCall { id, .. } = block {
                    tool_use_ids.insert(id.clone());
                }
            }
        }
    }

    // For every tool_use id, there must be a matching ToolResult (or error)
    // message later in the payload.  A missing entry means the prompt builder
    // failed to pair the call — Anthropic-compatible backends reject with 400.
    let mut answered_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in grace_payload_messages.iter() {
        if let UnifiedMessage::ToolResult { tool_call_id, .. } = msg {
            answered_ids.insert(tool_call_id.clone());
        }
    }

    let orphans: Vec<&String> = tool_use_ids
        .iter()
        .filter(|id| !answered_ids.contains(*id))
        .collect();

    assert!(
        orphans.is_empty(),
        "grace-turn payload has orphaned tool_use ids (no matching tool_result/tool_error): \
         {orphans:?}\n\
         grace payload messages:\n{grace_payload_messages:#?}"
    );
}

// =============================================================================
// Task-6 CAPSTONE — Fan-out → Thrash → Steer → Partial delivery
//
// Script (by non-grace provider call index):
//   Call 1 (fan-out): 5 distinct web_fetch tool_use blocks (args_hash 0..5).
//     Ring after: [wf0,wf1,wf2,wf3,wf4] (len=5). Tier-1: trailing_repeat_run=1 <
//     repeat_threshold=5. Tier-2: same_name_run=5 < TOOL_HISTORY_WINDOW=8. → Continue.
//   Call 2 (thrash): 8 file_read cycling 3 files (hashes 0,1,2,0,1,2,0,1).
//     Ring after: last 8 = all file_read. same_name_run=8 >= 8.
//     distinct=3, distinctness=3/8=0.375 < novelty_min=0.5. silent. → Veto #1.
//   Call 3 (thrash): same 8 file_read batch. Ring unchanged. → Veto #2.
//     verifier_veto_count(2) >= steer_max(2) → TerminateReason::VerifierVeto,
//     fires grace turn with GRACE_NUDGE_VERIFIER_VETO.
//   Grace turn: detect sentinel "safety cap has now stopped" → return text.
//
// Profile: {steer_max:2, ..conservative()} — silence_required=true so thrash
// must emit no assistant text.
// =============================================================================

/// Scripted provider that drives: fan-out turn → thrash turns → grace turn.
///
/// `calls` counts non-grace invocations (0-based). The grace turn is detected
/// by GRACE_NUDGE_VERIFIER_VETO's unique sentinel in the last user message.
struct FanoutThenThrashProvider {
    /// Total call count including the grace turn.
    calls: AtomicUsize,
    /// Records whether the grace turn was seen (for assertion #4).
    grace_seen: std::sync::atomic::AtomicBool,
}

impl FanoutThenThrashProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            grace_seen: std::sync::atomic::AtomicBool::new(false),
        })
    }
    fn total_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn grace_was_seen(&self) -> bool {
        self.grace_seen.load(Ordering::SeqCst)
    }
}

impl AiProvider for FanoutThenThrashProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        // Detect the veto-cap grace turn: the GRACE_NUDGE_VERIFIER_VETO sentinel
        // ends with a unique phrase about the "safety cap".
        let is_grace = payload
            .messages
            .last()
            .map(|m| m.text_content().contains("safety cap has now stopped"))
            .unwrap_or(false);
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if is_grace {
                self.grace_seen.store(true, Ordering::SeqCst);
                // Grace turn: return meaningful text so assertion #4 (non-empty
                // delivery) passes.
                return Ok(ProviderResponse::text_only(
                    "Based on the 5 pages I fetched: the news cycle shows three recurring \
                     themes — the task remains incomplete due to ambiguous scope."
                        .to_string(),
                ));
            }
            match n {
                // Call 0 (fan-out): 5 distinct web_fetch, all different args_hash.
                // Ring after: [wf0,wf1,wf2,wf3,wf4]. Tier-1: trailing_repeat=1 < 5.
                // Tier-2: same_name_run=5 < 8. → Continue (fan-out passes).
                0 => Ok(ProviderResponse {
                    text: None, // silent — not needed for fan-out
                    tool_calls: (0u64..5)
                        .map(|i| crate::providers::adapter::NativeToolCall {
                            thought_signature: None,
                            id: format!("wf-{i}"),
                            name: "web_fetch".to_string(),
                            arguments: serde_json::json!({"url": format!("https://example.com/page/{i}")}),
                        })
                        .collect(),
                    ..Default::default()
                }),
                // Calls 1 & 2 (thrash): 8 file_read cycling 3 files.
                // Each batch adds 8 to the ring, pushing out the 5 web_fetch.
                // After batch: ring = 8 file_read. same_name_run=8 >= 8.
                // distinct=3, distinctness=0.375 < 0.5. No text (silent). → Veto.
                _ => Ok(ProviderResponse {
                    text: None, // must be silent — silence_required=true
                    tool_calls: (0u64..8)
                        .map(|i| crate::providers::adapter::NativeToolCall {
                            thought_signature: None,
                            id: format!("fr-{n}-{i}"),
                            name: "file_read".to_string(),
                            arguments: serde_json::json!({"path": format!("/ref/file_{}.md", i % 3)}),
                        })
                        .collect(),
                    ..Default::default()
                }),
            }
        })
    }
    fn name(&self) -> &str {
        "fanout-then-thrash"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn weak_model_fanout_then_thrash_steers_and_delivers_partial() {
    // Profile: steer_max=2 so the grace turn fires after exactly 2 thrash vetoes.
    // All other fields stay conservative (repeat_threshold=5, halt_threshold=8,
    // novelty_min=0.5, silence_required=true).
    let profile = crate::verification::ModelRobustnessProfile {
        steer_max: 2,
        ..crate::verification::ModelRobustnessProfile::conservative()
    };

    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("fetch the latest news from these 5 sources and summarize"),
    ]);
    let provider = FanoutThenThrashProvider::new();
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(crate::verification::ToolLoopVerifier::new()))
            .build(),
    );
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: profile,
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        // High enough that the veto cap (after 2 thrash vetoes) fires first.
        max_iterations: Some(20),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = NoopHarnessCallback;
    harness
        .run(&sample_session_id(), &mut cb, &cancel)
        .await
        .expect("harness.run should complete via veto cap");

    // ── Assertion 1: fan-out was NOT vetoed ──────────────────────────────────
    // If the fan-out had been vetoed, total_calls would be ≤ 2 (fan-out call +
    // grace turn). We need at least 3 non-grace calls (fan-out + 2 thrash).
    // total_calls includes the grace turn, so ≥ 4 means fan-out + 2 thrash + grace.
    let total = provider.total_calls();
    assert!(
        total >= 4,
        "expected ≥4 provider calls (fan-out + 2 thrash + grace); got {total} — \
         fan-out may have been incorrectly vetoed",
    );

    // Also confirm via session events: no [verifier veto] message after call 0.
    // The first veto must be on call 1 (thrash), not call 0 (fan-out).
    let events = session.snapshot().await;
    let veto_messages: Vec<_> = events
        .iter()
        .filter(|r| {
            matches!(&r.event,
                SessionEvent::UserMessage { content, .. }
                if content.text.starts_with("[verifier veto]"))
        })
        .collect();
    // Exactly 2 veto messages (one per thrash call), none from the fan-out turn.
    assert_eq!(
        veto_messages.len(),
        2,
        "expected exactly 2 [verifier veto] messages (one per thrash call); got {}. \
         Fan-out should not have been vetoed.\nevents: {events:#?}",
        veto_messages.len(),
    );

    // ── Assertion 2: thrash was STEERED (VerifierVeto), not Halted ──────────
    // The key discriminator: Tier-2 emits Veto (steer path), not Halt.
    // TerminateReason must be VerifierVeto, never StopHookHalt.
    assert!(
        matches!(
            harness.terminate_reason(),
            crate::orchestrator::dispatch::TerminateReason::VerifierVeto { .. }
        ),
        "thrash must terminate via VerifierVeto (steer path, not Halt); got {:?}",
        harness.terminate_reason(),
    );

    // ── Assertion 3: terminated via the veto-cap grace path ─────────────────
    assert!(
        harness.hit_limit(),
        "hit_limit must be set on VerifierVeto termination (grace path fired)",
    );
    // The provider must have seen the grace-turn sentinel.
    assert!(
        provider.grace_was_seen(),
        "grace turn must have been fired (provider must detect the veto-cap nudge)",
    );

    // ── Assertion 4: partial delivery worked — grace text is NON-EMPTY ───────
    let grace_text_delivered = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. }
            if !content.text.is_empty() && content.text.contains("5 pages I fetched"))
    });
    assert!(
        grace_text_delivered,
        "grace turn text must reach the session log (non-empty partial delivery); \
         got events: {events:#?}",
    );
}

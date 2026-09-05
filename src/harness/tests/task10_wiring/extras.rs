//! Second half of `task10_wiring` integration tests — epoch registrar +
//! cap-grace + empty-then-text + later regressions. Shares mock types
//! with [`super`] via `use super::*;`.

use crate::sync_primitives::{Arc, AtomicBool, AtomicUsize, Ordering};
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::context::budget::ContextBudget;
use crate::error::Result as AlephResult;
use crate::harness::tests::harness_ext::AgentHarnessTestExt;
use crate::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::session::events::SessionEvent;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};
use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};
use crate::verification::{StopHookVerifier, VerifierChain};

use super::{
    sample_session_id, tiny_budget_config, turn_started_event, user_message_event,
    CountingProvider, FailingProvider, MockSession, NoopTools,
};

// =============================================================================
// Fake SessionEpochRegistrar helpers for split tests
// =============================================================================

/// Registrar that always succeeds, recording whether it was called.
struct OkRegistrar {
    called: Arc<AtomicBool>,
}

impl OkRegistrar {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            called: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl crate::session::epoch_registrar::SessionEpochRegistrar for OkRegistrar {
    async fn register_epoch(
        &self,
        _key: &crate::session::service::SessionId,
    ) -> anyhow::Result<()> {
        self.called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Registrar that always fails.
struct FailRegistrar;

#[async_trait]
impl crate::session::epoch_registrar::SessionEpochRegistrar for FailRegistrar {
    async fn register_epoch(
        &self,
        _key: &crate::session::service::SessionId,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("registrar deliberately fails"))
    }
}

// =============================================================================
// Test — SplitSession: circuit-breaker trip → split → run continues in child.
// =============================================================================
#[tokio::test]
async fn split_session_directive_continues_run_in_child_session() {
    // 80-char user message with budget=100 tokens → ratio=0.80, in warning zone
    // (warn=0.50, critical=0.90). circuit_breaker_max=1 → first record_compaction
    // trips it. max_splits=1 → SplitSession (not FinalReply).
    let user_text = "y".repeat(80);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    // Provider returns a short text answer → loop ends cleanly after one turn in child.
    let provider = CountingProvider::new("all done");

    let mut cfg = tiny_budget_config(100, 0.50, 0.90);
    cfg.circuit_breaker_max = 1;
    cfg.max_splits = 1;
    let budget = ContextBudget::new(&cfg);

    // FailingProvider drives the compactor's deterministic-truncation fallback path.
    let compactor = Arc::new(crate::context::compact::compactor::ContextCompactor::new(
        Arc::new(FailingProvider) as Arc<dyn AiProvider>,
        crate::context::compact::compactor::CompactorConfig {
            fresh_tail: 1,
            ..Default::default()
        },
    ));

    let registrar = OkRegistrar::new();

    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor),
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
        session_epoch_registrar: Some(
            registrar.clone() as Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>
        ),
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
        .expect("run should complete Ok after split");

    // The registrar was called, confirming the split path was taken.
    assert!(
        registrar.called.load(Ordering::SeqCst),
        "epoch registrar must have been called during split",
    );
    // After the split, run() must record the child session as the final id.
    let final_id = harness
        .final_session_id()
        .expect("final_session_id must be Some after a split");
    let parent = sample_session_id();
    let expected_child = parent.with_next_epoch();
    assert_eq!(
        final_id, expected_child,
        "final session must be parent.epoch+1",
    );
}

// =============================================================================
// Test — SplitSession fail-soft: registrar error → compact to fit and CONTINUE
// (never-break). The old hard-stop (hit_limit + ContextBudgetExhausted + grace)
// was removed; the fail-soft path now compacts in place and falls through to the
// normal LLM call, so the run finishes with a real answer, not a hard stop.
// =============================================================================
#[tokio::test]
async fn split_session_failsoft_compacts_and_continues() {
    // Same budget config as above — trips SplitSession on first warning turn.
    let user_text = "y".repeat(80);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("continued after compaction");

    let mut cfg = tiny_budget_config(100, 0.50, 0.90);
    cfg.circuit_breaker_max = 1;
    cfg.max_splits = 1;
    let budget = ContextBudget::new(&cfg);

    let compactor = Arc::new(crate::context::compact::compactor::ContextCompactor::new(
        Arc::new(FailingProvider) as Arc<dyn AiProvider>,
        crate::context::compact::compactor::CompactorConfig {
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
        context_compactor: Some(compactor),
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
        // FailRegistrar always returns Err → split fails → fail-soft compacts and continues.
        session_epoch_registrar: Some(Arc::new(FailRegistrar)
            as Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>),
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
        .expect("run must complete Ok even when split fails");

    // Never-break: a failed split must NOT hard-stop. The fail-soft path compacts
    // to fit and falls through to the normal LLM call, which produces the answer.
    assert!(
        !harness.hit_limit(),
        "split fail-soft must compact and continue, never set hit_limit",
    );
    assert_ne!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::ContextBudgetExhausted,
        "a failed split must never terminate the run on context budget",
    );
    assert_eq!(
        provider.call_count(),
        1,
        "the run must continue into exactly one normal LLM call after the failed split",
    );
    // No split happened → final session is same as parent (epoch unchanged).
    let final_id = harness.final_session_id();
    let parent = sample_session_id();
    assert!(
        final_id.is_none() || final_id.as_ref() == Some(&parent),
        "final session must equal parent when split fails; got {final_id:?}",
    );
}

/// Returns a `noop_tool` tool_call (unique id per call) for the first
/// `tool_call_turns` calls, then text-only "final summary". Drives the
/// `max_iterations` cap: the capped iterations emit tool calls (no
/// terminal text), and the grace turn — the call after the cap — gets
/// the text response.
struct CapGraceProvider {
    calls: AtomicUsize,
    tool_call_turns: usize,
}

impl CapGraceProvider {
    fn new(tool_call_turns: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            tool_call_turns,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for CapGraceProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.tool_call_turns {
                Ok(ProviderResponse {
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: format!("call-{n}"),
                        name: "noop_tool".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    ..Default::default()
                })
            } else {
                Ok(ProviderResponse::text_only("final summary".to_string()))
            }
        })
    }
    fn name(&self) -> &str {
        "cap-grace"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Which way the boundary grace call misbehaves — used to exercise
/// `fire_boundary_grace_turn`'s shared `race_llm_call` robustness from the surviving
/// `max_iterations` trigger (the budget trigger that used to reach it was removed).
#[derive(Clone, Copy)]
enum GraceCallOutcome {
    /// Return a provider error on the grace call (LLM-error fail-soft path).
    Fail,
    /// Hang forever on the grace call (turn-timeout abort path).
    Hang,
}

/// Emits `tool_call_turns` tool-call turns (no text) to exhaust the
/// `max_iterations` cap, then fails or hangs on the boundary grace call.
struct CapGraceRobustnessProvider {
    calls: AtomicUsize,
    tool_call_turns: usize,
    grace_outcome: GraceCallOutcome,
}

impl CapGraceRobustnessProvider {
    fn new(tool_call_turns: usize, grace_outcome: GraceCallOutcome) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            tool_call_turns,
            grace_outcome,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for CapGraceRobustnessProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.tool_call_turns {
                return Ok(ProviderResponse {
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: format!("call-{n}"),
                        name: "noop_tool".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    ..Default::default()
                });
            }
            match self.grace_outcome {
                GraceCallOutcome::Fail => Err(crate::error::AlephError::provider("grace fail")),
                GraceCallOutcome::Hang => {
                    std::future::pending::<()>().await;
                    unreachable!("pending future never resolves")
                }
            }
        })
    }
    fn name(&self) -> &str {
        "cap-grace-robustness"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Returns an all-empty response (no text, no tool_calls, no thinking) for
/// the first `empty_calls` calls, then text-only "recovered". Drives the
/// empty-response retry guard (H3).
struct EmptyThenTextProvider {
    calls: AtomicUsize,
    empty_calls: usize,
}

impl EmptyThenTextProvider {
    fn new(empty_calls: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            empty_calls,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for EmptyThenTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.empty_calls {
                Ok(ProviderResponse::default())
            } else {
                Ok(ProviderResponse::text_only("recovered".to_string()))
            }
        })
    }
    fn name(&self) -> &str {
        "empty-then-text"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Tool service that counts `execute` calls and always succeeds. Used to
/// prove the within-batch dedup memo (H4) does not span turns.
struct CountingTools {
    count: AtomicUsize,
}

impl CountingTools {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            count: AtomicUsize::new(0),
        })
    }
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolService for CountingTools {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(crate::session::events::ToolOutput {
            value: serde_json::Value::String("ok".to_string()),
            metadata: Default::default(),
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
// =============================================================================
// C1 — the `max_iterations` cap fires a grace turn so the user gets terminal
// text instead of an empty / mid-thought response.
// =============================================================================
#[tokio::test]
async fn max_iterations_cap_fires_grace_turn_for_terminal_text() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do work")]);
    // 2 tool-call turns, then text — the grace turn is provider call #3.
    let provider = CapGraceProvider::new(2);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        max_iterations: Some(2),
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
    harness
        .run(&sample_session_id(), &mut NoopHarnessCallback, &cancel)
        .await
        .expect("run should succeed when capped");

    assert!(harness.hit_limit(), "max_iterations cap must set hit_limit");
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::HitMaxIterations { used: 2 },
    );
    // 2 capped tool-call turns + 1 grace turn = 3 provider calls.
    assert_eq!(
        provider.call_count(),
        3,
        "the max_iterations cap must fire exactly one grace turn",
    );
    // The grace turn's text must reach the session log so the user sees it.
    let events = session.snapshot().await;
    let grace_text_present = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. } if content.text == "final summary")
    });
    assert!(
        grace_text_present,
        "grace turn text must be persisted; got: {events:#?}",
    );
}

// =============================================================================
// Re-homed from the removed budget→FinalReply trigger: a boundary grace call
// whose LLM errors must fail-soft — the harness still completes cleanly with
// hit_limit set and leaves no partial assistant event. Exercises the same
// `fire_boundary_grace_turn` → `race_llm_call` error path that budget pressure used
// to reach.
// =============================================================================
#[tokio::test]
async fn boundary_grace_turn_failsoft_on_llm_error() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do work")]);
    // 1 tool-call turn exhausts the cap → the grace turn is provider call #2, which fails.
    let provider = CapGraceRobustnessProvider::new(1, GraceCallOutcome::Fail);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        max_iterations: Some(1),
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
    harness
        .run(&sample_session_id(), &mut NoopHarnessCallback, &cancel)
        .await
        .expect("grace turn LLM failure must NOT bubble out of run");

    assert!(harness.hit_limit(), "max_iterations cap must set hit_limit");
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::HitMaxIterations { used: 1 },
    );
    assert_eq!(
        provider.call_count(),
        2,
        "1 capped tool-call turn + 1 (failing) grace turn = 2 provider calls",
    );
    // The failing grace call must not persist any terminal text — the user gets
    // no salvaged answer. (The capped tool-call turn legitimately emits one
    // text-less AssistantMessage carrying its tool_use; the grace failure adds none.)
    let events = session.snapshot().await;
    let terminal_text_present = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. } if !content.text.trim().is_empty())
    });
    assert!(
        !terminal_text_present,
        "a grace-call LLM error must not persist terminal text; got: {events:#?}",
    );
}

// =============================================================================
// H3 — a transient empty provider response is retried, not misreported as a
// clean completion; a persistently empty provider sets a distinct reason.
// =============================================================================
#[tokio::test]
async fn empty_response_retries_then_recovers() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let provider = EmptyThenTextProvider::new(1); // 1 empty, then text
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
    assert_eq!(state, TurnState::Done);
    assert_eq!(
        provider.call_count(),
        2,
        "one empty response must trigger exactly one retry",
    );
    let events = session.snapshot().await;
    let recovered = events.iter().any(|r| {
        matches!(&r.event,
            SessionEvent::AssistantMessage { content, .. } if content.text == "recovered")
    });
    assert!(recovered, "the retry must surface the recovered text");
}

#[tokio::test]
async fn empty_response_exhausted_sets_terminate_reason() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("hello")]);
    let provider = EmptyThenTextProvider::new(99); // never recovers
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
    assert_eq!(state, TurnState::Done);
    // 1 primary call + EMPTY_RESPONSE_RETRIES (2) retries = 3 calls, then give up.
    assert_eq!(
        provider.call_count(),
        3,
        "empty-response retries are bounded",
    );
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::EmptyResponseExhausted,
        "a persistently empty provider must not be reported as a clean completion",
    );
}

// =============================================================================
// H4 — the within-batch dedup memo does not span turns: an identical tool
// call in a later turn re-executes against fresh state instead of replaying
// a stale cached result.
// =============================================================================
#[tokio::test]
async fn tool_memo_does_not_span_turns() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("loop a tool"),
    ]);
    // 2 turns, each emitting an identical `noop_tool({})` call.
    let provider = CapGraceProvider::new(2);
    let tools = CountingTools::new();
    let deps = HarnessDeps {
        session: session.clone(),
        tools: tools.clone(),
        llm: provider.clone(),
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
        max_iterations: Some(2),
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
    harness
        .run(&sample_session_id(), &mut NoopHarnessCallback, &cancel)
        .await
        .expect("run should succeed");
    assert_eq!(
        tools.count(),
        2,
        "an identical tool call in a later turn must re-execute, not serve a stale cached result",
    );
}

// =============================================================================
// Re-homed from the removed budget→FinalReply trigger: a boundary grace call
// that hangs must abort on the turn-timeout, not hang the harness. Exercises the
// same `fire_boundary_grace_turn` → `race_llm_call` timeout path budget pressure used
// to reach.
// =============================================================================
#[tokio::test]
async fn boundary_grace_turn_times_out_instead_of_hanging() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("do work")]);
    // 1 tool-call turn exhausts the cap → the grace turn is provider call #2, which hangs.
    let provider = CapGraceRobustnessProvider::new(1, GraceCallOutcome::Hang);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        max_iterations: Some(1),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(std::time::Duration::from_millis(20)),
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let started = std::time::Instant::now();
    harness
        .run(&sample_session_id(), &mut NoopHarnessCallback, &cancel)
        .await
        .expect("a hung grace turn must not bubble out of run");
    let elapsed = started.elapsed();

    assert!(harness.hit_limit(), "max_iterations cap must set hit_limit");
    assert_eq!(
        harness.terminate_reason(),
        crate::orchestrator::dispatch::TerminateReason::HitMaxIterations { used: 1 },
    );
    assert_eq!(
        provider.call_count(),
        2,
        "1 capped tool-call turn + 1 (hanging) grace turn = 2 provider calls",
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the hung grace call must abort on the 20ms turn-timeout, not hang; took {elapsed:?}",
    );
}

// =============================================================================
// M10 — a compact-to-fit turn (critical pressure → CompactToFit → normal LLM
// call) folds its provider usage into BOTH total_tokens and the per-component
// breakdown, keeping the documented invariant. (Before the never-break change
// this exercised the budget grace turn; that path no longer fires on pressure.)
// =============================================================================
#[tokio::test]
async fn compact_to_fit_turn_keeps_token_breakdown_in_lockstep() {
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = Arc::new(super::super::stability::UsageTextProvider {
        usage: crate::providers::adapter::TokenUsage {
            input_tokens: 30,
            output_tokens: 12,
            cache_read_tokens: Some(4),
            cache_creation_tokens: Some(2),
            thinking_tokens: Some(7),
            cost: None,
        },
    });
    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider as Arc<dyn AiProvider>,
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
    // TurnState is must_use; this test inspects accumulated token usage
    // after the compact-to-fit turn's LLM call, not the loop-control signal.
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    let total = harness.total_tokens();
    let breakdown = harness.token_breakdown();
    assert!(
        total > 0,
        "the compact-to-fit turn must record provider usage"
    );
    assert_eq!(
        breakdown.total(),
        total,
        "breakdown.total() must stay in lockstep with total_tokens()",
    );
}

// =============================================================================
// Claude-Code harness parity G1 — Stop hook `Halt` verdict terminates the loop
// immediately (claude-code `preventContinuation: true` semantics). Distinct
// from `Block` (which forces Continue + retry).
// =============================================================================
struct AlwaysHaltHook {
    reason: String,
}

#[async_trait]
impl StopHookHandler for AlwaysHaltHook {
    fn name(&self) -> &str {
        "always-halt"
    }
    async fn evaluate(
        &self,
        _ctx: &StopHookContext,
        _cancel: &CancellationToken,
    ) -> StopHookVerdict {
        StopHookVerdict::Halt {
            reason: self.reason.clone(),
        }
    }
}

#[tokio::test]
async fn stop_hook_halt_terminates_loop_with_dedicated_reason() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("done?")]);
    let provider = CountingProvider::new("all done");

    let hooks: Arc<Vec<Arc<dyn StopHookHandler>>> = Arc::new(vec![Arc::new(AlwaysHaltHook {
        reason: "policy violation: halt".to_string(),
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
        TurnState::Done,
        "Halt must terminate the loop (Done) — distinct from Block (Continue)"
    );

    let reason = harness.terminate_reason();
    assert!(
        matches!(
            &reason,
            crate::orchestrator::dispatch::TerminateReason::StopHookHalt { reason: r }
                if r == "policy violation: halt"
        ),
        "Halt must set TerminateReason::StopHookHalt with hook's reason; got {:?}",
        reason
    );
    assert!(
        harness.hit_limit(),
        "Halt must flag hit_limit=true (intentional cap-style exit)",
    );

    // The halt reason is persisted as a UserMessage so transcript consumers
    // see it — same pattern as the Veto path, with a different prefix.
    let events = session.snapshot().await;
    let halt_injected = events.iter().any(|r| match &r.event {
        SessionEvent::UserMessage { content, .. } => {
            content.text.contains("[stop hook halt]")
                && content.text.contains("policy violation: halt")
        }
        _ => false,
    });
    assert!(
        halt_injected,
        "halt reason must be persisted as a UserMessage; got events: {:#?}",
        events
    );
}

// =============================================================================
// Claude-Code harness parity G2 — `max_output_tokens` recovery loop. Provider
// returns `StopReason::MaxTokens` with partial text for N turns, then EndTurn
// with final text. The harness must retry up to MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
// times and surface the final clean response — carrying every partial forward so
// the persisted answer (and the next turn's prompt, rebuilt from it) is the WHOLE
// answer, not just the continuation.
// =============================================================================

/// Records the deltas the harness pushed to the stream. Non-HTTP mock providers
/// never stream, so the whole answer must arrive in the one-shot emit.
#[derive(Default)]
struct DeltaCapture {
    deltas: Vec<String>,
}

impl crate::harness::HarnessCallback for DeltaCapture {
    fn on_delta(&mut self, text: &str) {
        self.deltas.push(text.to_string());
    }
}

/// Text of the last persisted `AssistantMessage`, i.e. what the next turn's
/// prompt gets rebuilt from.
fn last_assistant_text(events: &[crate::session::events::SessionEventRecord]) -> String {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .expect("the turn must persist an AssistantMessage")
}

struct MaxTokensThenTextProvider {
    calls: AtomicUsize,
    max_tokens_calls: usize,
}

impl MaxTokensThenTextProvider {
    fn new(max_tokens_calls: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            max_tokens_calls,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for MaxTokensThenTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.max_tokens_calls {
                // Partial text + MaxTokens stop reason — mirrors a provider
                // hitting its output-token cap mid-stream.
                Ok(ProviderResponse {
                    stop_reason: StopReason::MaxTokens,
                    ..ProviderResponse::text_only(format!("partial-{n}"))
                })
            } else {
                Ok(ProviderResponse::text_only(
                    "final clean response".to_string(),
                ))
            }
        })
    }
    fn name(&self) -> &str {
        "max-tokens-then-text"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn max_output_tokens_recovery_eventually_returns_clean_text() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("write a long answer"),
    ]);
    // 2 MaxTokens responses, then clean text — recovery should succeed
    // (RECOVERY_LIMIT=3 allows up to 3 retries).
    let provider = MaxTokensThenTextProvider::new(2);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let mut cb = DeltaCapture::default();
    let state = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn should succeed");

    assert_eq!(
        state,
        TurnState::Done,
        "after recovery, the clean text response stops the loop"
    );
    assert_eq!(
        provider.call_count(),
        3,
        "expected 1 initial + 2 retries; final call returned clean text",
    );
    let reason = harness.terminate_reason();
    assert!(
        matches!(
            reason,
            crate::orchestrator::dispatch::TerminateReason::Completed
        ),
        "recovery success must report Completed, not MaxOutputTokensExhausted; got {:?}",
        reason
    );

    // The half-answers the provider already generated must survive: the retries
    // push them onto a LOCAL message vec, so only an explicit carry keeps them in
    // the session log the next turn's prompt is rebuilt from.
    let whole = "partial-0partial-1final clean response";
    assert_eq!(
        last_assistant_text(&session.snapshot().await),
        whole,
        "persisted answer must be partials + continuation, not the continuation alone",
    );
    // Mock provider = no HTTP seam = non-streaming turn, so the user's only copy
    // of the answer is this one-shot emit. Exactly one delta: nothing may be
    // emitted from inside the recovery loop (that would bypass the output
    // guardrail stage).
    assert_eq!(
        cb.deltas,
        vec![whole.to_string()],
        "a non-streaming turn must deliver the whole answer in one delta",
    );
}

#[tokio::test]
async fn max_output_tokens_recovery_exhausted_sets_dedicated_terminate_reason() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("write forever"),
    ]);
    // 4 MaxTokens responses — exceeds RECOVERY_LIMIT=3, so the harness
    // gives up and reports MaxOutputTokensExhausted.
    let provider = MaxTokensThenTextProvider::new(10);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let mut cb = DeltaCapture::default();
    let _ = harness
        .run_turn(&sample_session_id(), &mut cb)
        .await
        .expect("run_turn should succeed");

    // MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3 in think.rs, so 1 initial call + 3 retries = 4.
    assert_eq!(
        provider.call_count(),
        4,
        "expected 1 initial + RECOVERY_LIMIT(=3) retries before giveup",
    );
    let reason = harness.terminate_reason();
    assert!(
        matches!(
            reason,
            crate::orchestrator::dispatch::TerminateReason::MaxOutputTokensExhausted
        ),
        "after exhausting recovery, terminate_reason must be MaxOutputTokensExhausted; got {:?}",
        reason
    );

    // Giving up is not the same as throwing the work away: every partial the
    // provider managed to emit is still persisted and still delivered.
    let whole = "partial-0partial-1partial-2partial-3";
    assert_eq!(
        last_assistant_text(&session.snapshot().await),
        whole,
        "exhausted recovery must still persist all partials, not only the last one",
    );
    assert_eq!(
        cb.deltas,
        vec![whole.to_string()],
        "exhausted recovery must still deliver all partials to the user",
    );
}

/// Exhausts the MaxTokens recovery on the FIRST turn but keeps a tool call in
/// the truncated response (so the loop continues), then answers cleanly on the
/// next turn. Drives the exit-point reason-fidelity regression below.
struct MaxTokensToolCallThenTextProvider {
    calls: AtomicUsize,
    max_tokens_calls: usize,
}

impl MaxTokensToolCallThenTextProvider {
    fn new(max_tokens_calls: usize) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            max_tokens_calls,
        })
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AiProvider for MaxTokensToolCallThenTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.max_tokens_calls {
                // Truncated mid-stream, but a complete tool call survived —
                // the loop must keep running it, not stop.
                Ok(ProviderResponse {
                    tool_calls: vec![NativeToolCall {
                        thought_signature: None,
                        id: format!("call-{n}"),
                        name: "noop_tool".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                    stop_reason: StopReason::MaxTokens,
                    ..ProviderResponse::text_only(format!("partial-{n}"))
                })
            } else {
                Ok(ProviderResponse::text_only(
                    "final clean response".to_string(),
                ))
            }
        })
    }
    fn name(&self) -> &str {
        "max-tokens-toolcall-then-text"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Exit-point reason fidelity: a mid-run turn that exhausts the MaxTokens
/// recovery but still carries tool calls keeps the loop alive; when a later
/// turn completes cleanly the run must report `Completed`, not the stale
/// `MaxOutputTokensExhausted` from the truncated intermediate turn.
#[tokio::test]
async fn max_output_tokens_mid_run_does_not_stain_terminate_reason() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("research then answer"),
    ]);
    // 4 MaxTokens+tool-call responses exhaust RECOVERY_LIMIT=3 on turn 1;
    // call 5 (turn 2) returns clean text.
    let provider = MaxTokensToolCallThenTextProvider::new(4);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        llm: provider.clone(),
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
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);

    // Turn 1: recovery exhausts, but the surviving tool call keeps the loop
    // alive — no terminate reason may be recorded yet.
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("turn 1 should succeed");
    assert_eq!(
        state,
        TurnState::Continue,
        "tool call must continue the run"
    );
    assert_eq!(
        provider.call_count(),
        4,
        "turn 1 = 1 initial + RECOVERY_LIMIT(=3) retries",
    );

    // Turn 2: clean text ends the run.
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("turn 2 should succeed");
    assert_eq!(state, TurnState::Done);

    let reason = harness.terminate_reason();
    assert!(
        matches!(
            reason,
            crate::orchestrator::dispatch::TerminateReason::Completed
        ),
        "a clean final turn must supersede the mid-run truncation; got {:?}",
        reason
    );
}

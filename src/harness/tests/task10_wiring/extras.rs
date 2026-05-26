//! Second half of `task10_wiring` integration tests — epoch registrar +
//! cap-grace + empty-then-text + later regressions. Shares mock types
//! with [`super`] via `use super::*;`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::context::budget::ContextBudget;
use crate::error::Result as AlephResult;
use crate::harness::{AgentHarness, Harness, HarnessDeps, NoopHarnessCallback, TurnState};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::sandbox::test_util::MockSandbox;
use crate::session::events::SessionEvent;
use crate::session::service::SessionId;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};
use crate::verification::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};
use crate::verification::{StopHookVerifier, VerifierChain};

use super::{
    assistant_message_event_with_text, noop_sandbox_output, sample_session_id, tiny_budget_config,
    turn_started_event, user_message_event, CountingProvider, FailingProvider, MockSession,
    NoopTools,
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor),
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
// Test — SplitSession fail-soft: registrar error → fall back to FinalReply.
// =============================================================================
#[tokio::test]
async fn split_session_failsoft_falls_back_to_final_reply() {
    // Same budget config as above — trips SplitSession on first warning turn.
    let user_text = "y".repeat(80);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("grace summary");

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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor),
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        // FailRegistrar always returns Err → split fails → fall back to FinalReply.
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

    assert!(
        harness.hit_limit(),
        "FinalReply fallback path must set hit_limit",
    );
    // No split → final session is same as parent (epoch unchanged).
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
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
// M9 — the grace turn races cancel + turn-timeout; a hung provider on the
// grace call must not hang the harness.
// =============================================================================
#[tokio::test]
async fn grace_turn_times_out_instead_of_hanging() {
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: Arc::new(super::super::stability::HangingProvider) as Arc<dyn AiProvider>,
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
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
    let started = std::time::Instant::now();
    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("grace turn timeout must not bubble out of run_turn");
    let elapsed = started.elapsed();
    assert_eq!(state, TurnState::Done);
    assert!(harness.hit_limit());
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "grace turn must abort on the 20ms turn-timeout, not hang; took {elapsed:?}",
    );
}

// =============================================================================
// M10 — a grace turn folds its provider usage into BOTH total_tokens and the
// per-component breakdown, keeping the documented invariant.
// =============================================================================
#[tokio::test]
async fn grace_turn_keeps_token_breakdown_in_lockstep() {
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider as Arc<dyn AiProvider>,
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    // TurnState is must_use; this test inspects accumulated token usage
    // after the grace turn fires, not the returned loop-control signal.
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed");
    let total = harness.total_tokens();
    let breakdown = harness.token_breakdown();
    assert!(total > 0, "the grace turn must record provider usage");
    assert_eq!(
        breakdown.total(),
        total,
        "grace turn breakdown.total() must stay in lockstep with total_tokens()",
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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
// times and surface the final clean response.
// =============================================================================
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
                Ok(ProviderResponse::text_only("final clean response".to_string()))
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
    let session = MockSession::new(vec![turn_started_event(), user_message_event("write a long answer")]);
    // 2 MaxTokens responses, then clean text — recovery should succeed
    // (RECOVERY_LIMIT=3 allows up to 3 retries).
    let provider = MaxTokensThenTextProvider::new(2);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
        "after recovery, the clean text response stops the loop"
    );
    assert_eq!(
        provider.call_count(),
        3,
        "expected 1 initial + 2 retries; final call returned clean text",
    );
    let reason = harness.terminate_reason();
    assert!(
        matches!(reason, crate::orchestrator::dispatch::TerminateReason::Completed),
        "recovery success must report Completed, not MaxOutputTokensExhausted; got {:?}",
        reason
    );
}

#[tokio::test]
async fn max_output_tokens_recovery_exhausted_sets_dedicated_terminate_reason() {
    let session = MockSession::new(vec![turn_started_event(), user_message_event("write forever")]);
    // 4 MaxTokens responses — exceeds RECOVERY_LIMIT=3, so the harness
    // gives up and reports MaxOutputTokensExhausted.
    let provider = MaxTokensThenTextProvider::new(10);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
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
        tool_signal_sink: Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    };
    let harness = AgentHarness::new(deps);
    let _ = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
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
}

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
use crate::verification::{StopHookVerifier, ToolLoopVerifier, VerifierChain};

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
    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
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

fn assistant_message_event_with_text(text: &str) -> SessionEvent {
    SessionEvent::AssistantMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text: text.to_string(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
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
// Test 1 — Budget FinalReply trips hit_limit and skips the grace turn when a
// prior assistant text already exists on the log.
// =============================================================================
#[tokio::test]
async fn budget_final_reply_skips_grace_turn_when_text_already_present() {
    // 100-char user message, budget=10, critical=0.50 → ratio ~= 10 → Critical.
    // Includes a prior assistant text — grace turn skips, LLM not called.
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event(&user_text),
        assistant_message_event_with_text("here is your answer"),
    ]);
    let provider = CountingProvider::new("should not fire");

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,

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
        "grace turn must skip when prior assistant text exists",
    );
}

// =============================================================================
// Test 1b — Grace turn FIRES on FinalReply when no assistant turn has produced
// displayable text yet (would otherwise leave the user with a mid-thought hang).
// =============================================================================
#[tokio::test]
async fn budget_final_reply_fires_grace_turn_when_no_prior_text() {
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("grace turn summary");

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed on FinalReply");

    assert_eq!(state, TurnState::Done);
    assert!(harness.hit_limit());
    assert_eq!(
        provider.call_count(),
        1,
        "grace turn must fire exactly once when no prior assistant text exists"
    );

    // The grace-turn assistant message must be on the log so the user sees it.
    let events = session.snapshot().await;
    let grace_text_present = events.iter().any(|r| match &r.event {
        SessionEvent::AssistantMessage { content, .. } => content.text == "grace turn summary",
        _ => false,
    });
    assert!(
        grace_text_present,
        "grace turn LLM response must be persisted as an AssistantMessage; got: {:#?}",
        events
    );
}

// Test 1c (tool_use-only prior assistant message) covered as a focused unit
// test on `last_assistant_has_text` in `src/harness/agent/think.rs` — driving
// it as an integration test depends on the prompt-builder's handling of an
// unmatched `tool_use`, which is not the behavior under test here.

// =============================================================================
// Test 1d — Grace turn fail-soft: LLM errors during the grace call must not
// panic / propagate; harness still completes cleanly with hit_limit set.
// =============================================================================
#[tokio::test]
async fn budget_final_reply_grace_turn_failsoft_on_llm_error() {
    let user_text = "x".repeat(100);
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);

    let budget = ContextBudget::new(&tiny_budget_config(10, 0.40, 0.50));
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        // FailingProvider returns Err on every call, simulating "even the grace
        // call fails (still out of context, network down, etc.)". The harness
        // must swallow and complete.
        llm: Arc::new(FailingProvider) as Arc<dyn AiProvider>,
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("grace turn LLM failure must NOT bubble out of run_turn");

    assert_eq!(state, TurnState::Done);
    assert!(harness.hit_limit());
    // No assistant message emitted (grace turn failed before persistence).
    let events = session.snapshot().await;
    let assistant_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(
        assistant_count, 0,
        "grace turn LLM error must not leave a partial assistant event"
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
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: Some(compactor.clone()),
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,

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
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: Some(chain),
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,

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
// Test 6 — StopDiminishing fires grace turn + hit_limit when
// DiminishingReturnsDetector trips on an unproductive turn.
// Cycle 3 — after_turn was dead-wired before this commit.
// =============================================================================
#[tokio::test]
async fn diminishing_returns_fires_grace_and_hits_limit() {
    let user_text = "ping".to_string();
    let session = MockSession::new(vec![turn_started_event(), user_message_event(&user_text)]);
    let provider = CountingProvider::new("grace summary text");

    // budget pressure never trips (large budget, high thresholds)
    let mut cfg = tiny_budget_config(10_000, 0.99, 0.99);
    // window=1: one unproductive turn is enough to trip the detector.
    // threshold=10_000: any output below 10_000 tokens counts as diminishing.
    // CountingProvider returns no usage, so output_tokens=0 → trips.
    cfg.diminishing_window = 1;
    cfg.diminishing_threshold = 10_000;
    let budget = ContextBudget::new(&cfg);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTools),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider.clone(),
        verifier_chain: None,
        context_budget: Some(Arc::new(AsyncMutex::new(budget))),
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let state = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await
        .expect("run_turn should succeed on StopDiminishing");

    assert_eq!(state, TurnState::Done, "StopDiminishing must produce TurnState::Done");
    assert!(harness.hit_limit(), "hit_limit must be set when DiminishingReturnsDetector trips");
    assert_eq!(
        provider.call_count(),
        2,
        "1 primary call + 1 grace turn = 2 LLM calls expected",
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
    // MAX_VERIFIER_VETOS=10 safety cap.
    let chain = Arc::new(
        VerifierChain::builder()
            .with(Arc::new(ToolLoopVerifier::new()))
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
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: Some(7),
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,

    };
    let harness = AgentHarness::new(deps);
    let cancel = CancellationToken::new();
    let mut cb = crate::harness::NoopHarnessCallback;
    let _ = harness
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
// Test — Per-tool budget fires before harness-wide turn_timeout. The sleeping
// tool's describe() advertises max_duration_ms=50; the harness turn_timeout is
// 60s. The inner per-tool cap must win → StalledTurn in <500ms. Cycle 3.
// =============================================================================

/// A `ToolService` whose `describe()` advertises a 50ms per-tool budget and
/// whose `execute()` sleeps 200ms — long enough that the 50ms budget fires.
struct SleepyBudgetedTool;

#[async_trait]
impl ToolService for SleepyBudgetedTool {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, ToolError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Ok(crate::session::events::ToolOutput {
            value: serde_json::json!({"ok": true}),
            metadata: Default::default(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        Some(ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: crate::tools::service::ToolSource::Builtin,
            metadata: crate::tools::service::ToolDefinitionMetadata {
                max_duration_ms: Some(50),
                ..Default::default()
            },
        })
    }
    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        std::sync::Arc::from([])
    }
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
async fn per_tool_budget_fires_before_global_turn_timeout() {
    let session = MockSession::new(vec![
        turn_started_event(),
        user_message_event("call the slow tool"),
    ]);
    let provider = Arc::new(OneShotSleepyCallProvider);
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(SleepyBudgetedTool),
        sandbox: MockSandbox::new(noop_sandbox_output()),
        llm: provider as Arc<dyn AiProvider>,
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: crate::harness::chain_context::ChainContext::default(),
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: Some(std::time::Duration::from_secs(60)),
        turn_budget: None,
        result_store: None,
    };
    let harness = AgentHarness::new(deps);

    let started = std::time::Instant::now();
    let result = harness
        .run_turn(&sample_session_id(), &mut NoopHarnessCallback)
        .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(
            &result,
            Err(crate::harness::trait_def::HarnessError::StalledTurn {
                phase: crate::harness::trait_def::TurnPhase::Act { .. },
                ..
            })
        ),
        "expected StalledTurn in Act phase, got: {result:?}",
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "per-tool 50ms budget must fire well before the 60s global; saw {elapsed:?}",
    );
}

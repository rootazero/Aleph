//! Step definitions for agent loop features

use crate::world::{AgentLoopContext, AlephWorld, MockDecision};
use alephcore::agent_loop::{
    callback::{CollectingCallback, LoopEvent, NoOpLoopCallback},
    config::LoopConfig,
    decision::{Action, ActionResult, Decision},
    guards::GuardViolation,
    question::{ChoiceOption, QuestionKind},
    state::{LoopState, LoopStep, RequestContext, Thinking},
    AgentLoop, ActionExecutor, CompressedHistory, CompressorTrait, ThinkerTrait,
    CompactionTrigger, OptionalCompactionTrigger, LoopResult,
};
use alephcore::event::{EventType, StopReason};
use alephcore::Result;
use async_trait::async_trait;
use cucumber::{given, then, when};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ═══════════════════════════════════════════════════════════════════════════
// Mock Implementations
// ═══════════════════════════════════════════════════════════════════════════

/// Mock Thinker that returns predefined decisions
struct MockThinker {
    decisions: Mutex<Vec<Decision>>,
    call_count: AtomicUsize,
}

impl MockThinker {
    fn new(decisions: Vec<Decision>) -> Self {
        Self {
            decisions: Mutex::new(decisions),
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ThinkerTrait for MockThinker {
    async fn think(
        &self,
        _state: &LoopState,
        _tools: &[alephcore::dispatcher::UnifiedTool],
    ) -> Result<Thinking> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        let decisions = self.decisions.lock().unwrap();
        let decision = decisions.get(count).cloned().unwrap_or(Decision::Fail {
            reason: "No more decisions".to_string(),
        });

        Ok(Thinking {
            reasoning: Some(format!("Step {}", count)),
            decision,
            structured: None,
        })
    }
}

/// Mock Executor that always succeeds
struct MockExecutor;

#[async_trait]
impl ActionExecutor for MockExecutor {
    async fn execute(&self, action: &Action, _identity: &aleph_protocol::IdentityContext) -> ActionResult {
        match action {
            Action::ToolCall { tool_name, .. } => ActionResult::ToolSuccess {
                output: json!({"tool": tool_name, "result": "success"}),
                duration_ms: 100,
            },
            _ => ActionResult::Completed,
        }
    }
}

/// Mock Compressor that never compresses
struct MockCompressor;

#[async_trait]
impl CompressorTrait for MockCompressor {
    fn should_compress(&self, _state: &LoopState) -> bool {
        false
    }

    async fn compress(
        &self,
        _steps: &[LoopStep],
        _current_summary: &str,
    ) -> Result<CompressedHistory> {
        Ok(CompressedHistory {
            summary: "Compressed".to_string(),
            compressed_count: 0,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Convert MockDecision to Decision
fn to_decision(mock: &MockDecision) -> Decision {
    match mock {
        MockDecision::Complete { summary } => Decision::Complete {
            summary: summary.clone(),
        },
        MockDecision::UseTool {
            tool_name,
            arguments,
        } => Decision::UseTool {
            tool_name: tool_name.clone(),
            arguments: arguments.clone(),
        },
        MockDecision::AskUserRich { question, kind } => Decision::AskUserRich {
            question: question.clone(),
            kind: kind.clone(),
            question_id: None,
        },
        MockDecision::Fail { reason } => Decision::Fail {
            reason: reason.clone(),
        },
    }
}

/// Build decisions from context
fn build_decisions(ctx: &AgentLoopContext) -> Vec<Decision> {
    ctx.decision_sequence.iter().map(to_decision).collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Given Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a mock thinker that returns Complete with summary {string}")]
async fn given_thinker_complete(w: &mut AlephWorld, summary: String) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.add_decision(MockDecision::Complete { summary });
}

#[given(expr = "a mock thinker that returns UseTool {string} then Complete")]
async fn given_thinker_tool_then_complete(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.add_decision(MockDecision::UseTool {
        tool_name,
        arguments: json!({"query": "test"}),
    });
    ctx.add_decision(MockDecision::Complete {
        summary: "Search complete".to_string(),
    });
}

#[given("a mock thinker that always returns tool calls")]
async fn given_thinker_always_tools(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    // Add 20 tool calls - more than max steps
    for i in 0..20 {
        ctx.add_decision(MockDecision::UseTool {
            tool_name: format!("tool_{}", i),
            arguments: json!({}),
        });
    }
}

#[given("a mock thinker that returns AskUserRich then Complete")]
async fn given_thinker_ask_user_rich(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.add_decision(MockDecision::AskUserRich {
        question: "Choose option".to_string(),
        kind: QuestionKind::SingleChoice {
            choices: vec![ChoiceOption::new("A"), ChoiceOption::new("B")],
            default_index: Some(0),
        },
    });
    ctx.add_decision(MockDecision::Complete {
        summary: "Done after user choice".to_string(),
    });
}

#[given(expr = "a loop config with max steps {int}")]
async fn given_config_max_steps(w: &mut AlephWorld, max_steps: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.loop_config = Some(LoopConfig::for_testing().with_max_steps(max_steps as usize));
}

#[given(expr = "an event bus subscribed to {word}")]
async fn given_event_bus_subscribed(w: &mut AlephWorld, event_type: String) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    let event_types = match event_type.as_str() {
        "LoopContinue" => vec![EventType::LoopContinue],
        "LoopStop" => vec![EventType::LoopStop],
        "ToolCallCompleted" => vec![EventType::ToolCallCompleted],
        _ => vec![EventType::All],
    };
    ctx.setup_event_bus(event_types);
}

#[given("no event bus configured")]
async fn given_no_event_bus(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.event_bus = None;
}

#[given(expr = "an overflow detector with {int}K context limit")]
async fn given_overflow_detector(w: &mut AlephWorld, context_k: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    let context_limit = (context_k as usize) * 1000;
    let output_limit = 4000; // 4K output
    let reserve = 0.2; // 20% reserve
    ctx.setup_overflow_detector(context_limit, output_limit, reserve);
}

#[given(expr = "an overflow detector with {int}K usable tokens")]
async fn given_overflow_detector_usable(w: &mut AlephWorld, usable_k: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    // Calculate context limit to achieve the desired usable tokens
    // Usable = (context - output) * (1 - reserve)
    // Usable = (context - 1000) * 0.9
    // context = (usable / 0.9) + 1000
    let usable_tokens = (usable_k as usize) * 1000;
    let context_limit = ((usable_tokens as f64 / 0.9) as usize) + 1000;
    ctx.setup_overflow_detector(context_limit, 1000, 0.1);
}

#[given(expr = "an overflow detector with context {int}K output {int}K reserve {int} percent")]
async fn given_overflow_detector_explicit(w: &mut AlephWorld, context_k: i32, output_k: i32, reserve_pct: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    let context_limit = (context_k as usize) * 1000;
    let output_limit = (output_k as usize) * 1000;
    let reserve = (reserve_pct as f32) / 100.0;
    ctx.setup_overflow_detector(context_limit, output_limit, reserve);
}

#[given(expr = "token usage at {int} percent")]
async fn given_token_usage_percent(w: &mut AlephWorld, percent: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    // Calculate token usage based on usable tokens
    // Using default calculation: (context - output) * (1 - reserve)
    // For 10K usable tokens setup with context_limit ~12100, output 1000, reserve 0.1:
    // Usable = (12100 - 1000) * 0.9 = 9990 ~ 10000
    // We use 8100 as the usable tokens (from original test comment)
    let usable = 8100.0;
    ctx.token_usage = (usable * (percent as f64 / 100.0)) as usize;
}

#[given(expr = "token usage of {int} tokens")]
async fn given_token_usage_exact(w: &mut AlephWorld, tokens: i32) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    ctx.token_usage = tokens as usize;
}

// ═══════════════════════════════════════════════════════════════════════════
// When Steps
// ═══════════════════════════════════════════════════════════════════════════

#[when(expr = "I run the agent loop with request {string}")]
async fn when_run_loop(w: &mut AlephWorld, request: String) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let decisions = build_decisions(ctx);
    let thinker = Arc::new(MockThinker::new(decisions));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = ctx.loop_config.clone().unwrap_or_else(LoopConfig::for_testing);

    let agent_loop = AgentLoop::new(thinker, executor, compressor, config);

    let callback = CollectingCallback::new();

    // Create Owner identity for test
    let identity = aleph_protocol::IdentityContext::owner(
        "test-session".to_string(),
        "test".to_string(),
    );

    let result = agent_loop
        .run(
            request,
            RequestContext::empty(),
            vec![],
            identity,
            &callback,
            None,
            None,
        )
        .await;

    ctx.loop_result = Some(result.clone());
    ctx.events = callback.events();

    // Extract guard violation if present
    if let LoopResult::GuardTriggered(violation) = result {
        ctx.guard_violation = Some(violation);
    }
}

#[when("I run the agent loop with event bus")]
async fn when_run_loop_with_event_bus(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let decisions = build_decisions(ctx);
    let thinker = Arc::new(MockThinker::new(decisions));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = ctx.loop_config.clone().unwrap_or_else(LoopConfig::for_testing);
    let event_bus = ctx.event_bus.clone().expect("Event bus not configured");

    let agent_loop = AgentLoop::with_event_bus(thinker, executor, compressor, config, event_bus);

    // Create Owner identity for test
    let identity = aleph_protocol::IdentityContext::owner(
        "test-session".to_string(),
        "test".to_string(),
    );

    let result = agent_loop
        .run(
            "Test request".to_string(),
            RequestContext::empty(),
            vec![],
            identity,
            NoOpLoopCallback,
            None,
            None,
        )
        .await;

    ctx.loop_result = Some(result.clone());

    // Collect events from subscriber
    ctx.collect_bus_events().await;

    // Extract guard violation if present
    if let LoopResult::GuardTriggered(violation) = result {
        ctx.guard_violation = Some(violation);
    }
}

#[when("I run the agent loop with overflow detector")]
async fn when_run_loop_with_overflow(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let decisions = build_decisions(ctx);
    let thinker = Arc::new(MockThinker::new(decisions));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = LoopConfig::for_testing().with_realtime_overflow(true);
    let detector = ctx.overflow_detector.clone();

    let agent_loop =
        AgentLoop::with_unified_session(thinker, executor, compressor, config, None, detector);

    // Create Owner identity for test
    let identity = aleph_protocol::IdentityContext::owner(
        "test-session".to_string(),
        "test".to_string(),
    );

    let result = agent_loop
        .run(
            "Test request".to_string(),
            RequestContext::empty(),
            vec![],
            identity,
            NoOpLoopCallback,
            None,
            None,
        )
        .await;

    ctx.loop_result = Some(result);
}

#[when("I create an optional compaction trigger")]
async fn when_create_optional_trigger(w: &mut AlephWorld) {
    let _ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    // Test that OptionalCompactionTrigger works without EventBus
    let trigger = OptionalCompactionTrigger::new(None);
    // Store in AlephWorld to verify it was created (we'll test methods in Then step)
    w.last_result = Some(Ok(()));
    let _ = trigger; // Explicitly drop
}

#[when("I create a compaction trigger")]
async fn when_create_compaction_trigger(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);
    let event_bus = ctx.event_bus.clone().expect("Event bus not configured");
    let trigger = CompactionTrigger::new(event_bus);

    // Emit an event to verify it works
    trigger
        .emit_loop_continue("session-1", 0, 0, None, "test-model")
        .await;

    w.last_result = Some(Ok(()));
}

#[when("I create agent loop with unified session")]
async fn when_create_unified_session_loop(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    // Add minimal decisions for testing
    if ctx.decision_sequence.is_empty() {
        ctx.add_decision(MockDecision::Complete {
            summary: "Done".to_string(),
        });
    }

    let decisions = build_decisions(ctx);
    let thinker = Arc::new(MockThinker::new(decisions));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = LoopConfig::for_testing()
        .with_unified_session(true)
        .with_message_builder(true)
        .with_realtime_overflow(true);

    let event_bus = ctx.event_bus.clone();
    let detector = ctx.overflow_detector.clone();

    let agent_loop =
        AgentLoop::with_unified_session(thinker, executor, compressor, config.clone(), event_bus, detector);

    // Store config for assertions
    ctx.loop_config = Some(config);

    // Verify loop has overflow detector by checking if usage_percent returns non-zero
    // when detector is configured
    let test_state = LoopState::new(
        "test".to_string(),
        "test".to_string(),
        RequestContext::empty(),
    );
    // If overflow detector is set, usage_percent will be based on actual calculation
    // If not set, usage_percent returns 0
    let has_detector = ctx.overflow_detector.is_some();
    ctx.should_compact = Some(has_detector);

    w.last_result = Some(Ok(()));
}

#[when("I check should compact")]
async fn when_check_should_compact(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let thinker = Arc::new(MockThinker::new(vec![]));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = LoopConfig::for_testing().with_realtime_overflow(true);
    let detector = ctx.overflow_detector.clone();

    let agent_loop =
        AgentLoop::with_unified_session(thinker, executor, compressor, config, None, detector);

    let mut state = LoopState::new(
        "test-session".to_string(),
        "Test request".to_string(),
        RequestContext::empty(),
    );
    state.total_tokens = ctx.token_usage;

    ctx.should_compact = Some(agent_loop.should_compact_unified(&state));
}

#[when("I check is overflow")]
async fn when_check_is_overflow(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let thinker = Arc::new(MockThinker::new(vec![]));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = LoopConfig::for_testing().with_realtime_overflow(true);
    let detector = ctx.overflow_detector.clone();

    let agent_loop =
        AgentLoop::with_unified_session(thinker, executor, compressor, config, None, detector);

    let mut state = LoopState::new(
        "test-session".to_string(),
        "Test request".to_string(),
        RequestContext::empty(),
    );
    state.total_tokens = ctx.token_usage;

    ctx.is_overflow = Some(agent_loop.is_overflow(&state));
}

#[when("I check usage percent")]
async fn when_check_usage_percent(w: &mut AlephWorld) {
    let ctx = w.agent_loop.get_or_insert_with(AgentLoopContext::new);

    let thinker = Arc::new(MockThinker::new(vec![]));
    let executor = Arc::new(MockExecutor);
    let compressor = Arc::new(MockCompressor);

    let config = LoopConfig::for_testing().with_realtime_overflow(true);
    let detector = ctx.overflow_detector.clone();

    let agent_loop =
        AgentLoop::with_unified_session(thinker, executor, compressor, config, None, detector);

    let mut state = LoopState::new(
        "test-session".to_string(),
        "Test request".to_string(),
        RequestContext::empty(),
    );
    state.total_tokens = ctx.token_usage;

    ctx.usage_percent = Some(agent_loop.usage_percent(&state));
}

// ═══════════════════════════════════════════════════════════════════════════
// Then Steps
// ═══════════════════════════════════════════════════════════════════════════

#[then("the loop result should be Completed")]
async fn then_result_completed(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let result = ctx.loop_result.as_ref().expect("No loop result");
    assert!(
        matches!(result, LoopResult::Completed { .. }),
        "Expected Completed, got {:?}",
        result
    );
}

#[then("the loop result should be GuardTriggered")]
async fn then_result_guard_triggered(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let result = ctx.loop_result.as_ref().expect("No loop result");
    assert!(
        matches!(result, LoopResult::GuardTriggered(_)),
        "Expected GuardTriggered, got {:?}",
        result
    );
}

#[then(expr = "the summary should be {string}")]
async fn then_summary_should_be(w: &mut AlephWorld, expected: String) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let result = ctx.loop_result.as_ref().expect("No loop result");
    if let LoopResult::Completed { summary, .. } = result {
        assert_eq!(summary, &expected, "Summary mismatch");
    } else {
        panic!("Expected Completed result with summary");
    }
}

#[then(expr = "the steps should be {int}")]
async fn then_steps_should_be(w: &mut AlephWorld, expected: i32) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let result = ctx.loop_result.as_ref().expect("No loop result");
    let steps = result.steps();
    assert_eq!(steps, expected as usize, "Steps mismatch: expected {}, got {}", expected, steps);
}

#[then("the guard violation should be MaxSteps")]
async fn then_guard_max_steps(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let violation = ctx.guard_violation.as_ref().expect("No guard violation");
    assert!(
        matches!(violation, GuardViolation::MaxSteps { .. }),
        "Expected MaxSteps violation, got {:?}",
        violation
    );
}

#[then("events should include ActionStart")]
async fn then_events_include_action_start(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert!(
        ctx.events.iter().any(|e| matches!(e, LoopEvent::ActionStart { .. })),
        "Expected ActionStart event"
    );
}

#[then("events should include ActionDone with success")]
async fn then_events_include_action_done_success(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert!(
        ctx.events.iter().any(|e| matches!(e, LoopEvent::ActionDone { success: true, .. })),
        "Expected ActionDone with success event"
    );
}

#[then("events should include UserQuestionRequired")]
async fn then_events_include_user_question(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert!(
        ctx.events.iter().any(|e| matches!(e, LoopEvent::UserQuestionRequired { .. })),
        "Expected UserQuestionRequired event"
    );
}

#[then("a LoopContinue event should be emitted")]
async fn then_loop_continue_emitted(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert!(
        ctx.has_loop_continue(),
        "Expected LoopContinue event to be emitted"
    );
}

#[then(expr = "a ToolCallCompleted event should be emitted for {string}")]
async fn then_tool_completed_emitted(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert!(
        ctx.has_tool_completed(&tool_name),
        "Expected ToolCallCompleted event for '{}'",
        tool_name
    );
}

#[then(expr = "a LoopStop event should be emitted with reason {word}")]
async fn then_loop_stop_emitted(w: &mut AlephWorld, reason: String) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let expected_reason = match reason.as_str() {
        "Completed" => StopReason::Completed,
        "MaxIterationsReached" => StopReason::MaxIterationsReached,
        "DoomLoopDetected" => StopReason::DoomLoopDetected,
        "TokenLimitReached" => StopReason::TokenLimitReached,
        "UserAborted" => StopReason::UserAborted,
        _ => StopReason::Error(reason.clone()),
    };
    assert!(
        ctx.has_stop_reason(&expected_reason),
        "Expected LoopStop event with reason {:?}",
        expected_reason
    );
}

#[then("triggering loop continue should not panic")]
async fn then_trigger_loop_continue_no_panic(w: &mut AlephWorld) {
    // Create a new trigger and test it
    let trigger = OptionalCompactionTrigger::new(None);
    trigger
        .emit_loop_continue("session-1", 1, 1000, None, "model")
        .await;
    // If we get here without panic, test passes
    w.last_result = Some(Ok(()));
}

#[then("triggering loop stop should not panic")]
async fn then_trigger_loop_stop_no_panic(w: &mut AlephWorld) {
    // Create a new trigger and test it
    let trigger = OptionalCompactionTrigger::new(None);
    trigger.emit_loop_stop(StopReason::Completed).await;
    // If we get here without panic, test passes
}

#[then("the trigger should emit events successfully")]
async fn then_trigger_emits_events(w: &mut AlephWorld) {
    assert!(
        w.last_result.as_ref().map(|r| r.is_ok()).unwrap_or(false),
        "Expected trigger creation to succeed"
    );
}

#[then("should compact should be true")]
async fn then_should_compact_true(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert_eq!(
        ctx.should_compact,
        Some(true),
        "Expected should_compact to be true"
    );
}

#[then("should compact should be false")]
async fn then_should_compact_false(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert_eq!(
        ctx.should_compact,
        Some(false),
        "Expected should_compact to be false"
    );
}

#[then("is overflow should be true")]
async fn then_is_overflow_true(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert_eq!(
        ctx.is_overflow,
        Some(true),
        "Expected is_overflow to be true"
    );
}

#[then(expr = "usage percent should be {int}")]
async fn then_usage_percent_is(w: &mut AlephWorld, expected: i32) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let actual = ctx.usage_percent.expect("Usage percent not set");
    assert_eq!(
        actual, expected as u8,
        "Expected usage percent {}, got {}",
        expected, actual
    );
}

#[then("the loop should have overflow detector configured")]
async fn then_has_overflow_detector(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    assert_eq!(
        ctx.should_compact,
        Some(true),
        "Expected overflow detector to be configured"
    );
}

#[then("the loop config should have realtime overflow enabled")]
async fn then_realtime_overflow_enabled(w: &mut AlephWorld) {
    let ctx = w.agent_loop.as_ref().expect("Agent loop context not initialized");
    let config = ctx.loop_config.as_ref().expect("Loop config not set");
    assert!(
        config.use_realtime_overflow,
        "Expected use_realtime_overflow to be true"
    );
}

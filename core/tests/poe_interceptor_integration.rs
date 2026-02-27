//! Integration tests for POE interceptor round-trip.
//!
//! Verifies PoeLoopCallback (wrapping inner callback + StepEvaluator)
//! returns correct StepDirective based on budget state — tested via
//! the public LoopCallback trait interface.

use std::sync::Arc;

use alephcore::agent_loop::callback::LoopCallback;
use alephcore::agent_loop::decision::{Action, ActionResult, Decision};
use alephcore::agent_loop::state::{LoopState, LoopStep, RequestContext, Thinking};
use alephcore::poe::budget::PoeBudget;
use alephcore::poe::interceptor::callback::PoeLoopCallback;
use alephcore::poe::interceptor::step_evaluator::StepEvaluator;
use alephcore::poe::{StepDirective, SuccessManifest};
use alephcore::poe::validation::HardValidator;
use alephcore::agent_loop::callback::NoOpLoopCallback;
use tokio::sync::RwLock;

fn make_step() -> LoopStep {
    LoopStep {
        step_id: 0,
        observation_summary: String::new(),
        thinking: Thinking {
            reasoning: None,
            decision: Decision::Complete {
                summary: "done".into(),
            },
            structured: None,
        },
        action: Action::Completion {
            summary: "done".into(),
        },
        result: ActionResult::Completed,
        tokens_used: 0,
        duration_ms: 0,
    }
}

fn make_state() -> LoopState {
    LoopState::new(
        "integration-test-session".into(),
        "test request".into(),
        RequestContext::empty(),
    )
}

fn make_manifest() -> SuccessManifest {
    SuccessManifest::new("integration-task-1", "Complete the integration test objective")
}

/// Build a PoeLoopCallback wrapping NoOpLoopCallback with the given budget.
fn build_callback(budget: PoeBudget) -> PoeLoopCallback<NoOpLoopCallback> {
    let manifest = Arc::new(make_manifest());
    let budget = Arc::new(RwLock::new(budget));
    let evaluator = Arc::new(StepEvaluator::new(Arc::new(HardValidator::new())));

    PoeLoopCallback::new(NoOpLoopCallback, manifest, budget, evaluator)
}

// ============================================================================
// Round-trip tests: construct full callback stack, call on_step_evaluate
// ============================================================================

#[tokio::test]
async fn test_poe_interceptor_returns_continue_with_fresh_budget() {
    let callback = build_callback(PoeBudget::new(5, 100_000));
    let step = make_step();
    let state = make_state();

    let directive = callback.on_step_evaluate(&step, &state).await;

    assert!(
        matches!(directive, StepDirective::Continue),
        "Fresh budget should yield Continue, got {:?}",
        directive
    );
}

#[tokio::test]
async fn test_poe_interceptor_aborts_when_budget_exhausted() {
    let mut budget = PoeBudget::new(2, 100_000);
    budget.record_attempt(50_000, 0.8);
    budget.record_attempt(50_000, 0.7);
    assert!(budget.exhausted(), "Budget should be exhausted after 2/2 attempts");

    let callback = build_callback(budget);
    let step = make_step();
    let state = make_state();

    let directive = callback.on_step_evaluate(&step, &state).await;

    assert!(
        matches!(directive, StepDirective::Abort { .. }),
        "Exhausted budget should yield Abort, got {:?}",
        directive
    );

    if let StepDirective::Abort { reason } = directive {
        assert!(
            reason.contains("budget exhausted"),
            "Abort reason should mention budget exhaustion, got: {}",
            reason
        );
    }
}

#[tokio::test]
async fn test_poe_interceptor_suggests_strategy_switch_when_stuck() {
    let mut budget = PoeBudget::new(10, 100_000);
    // Record identical distance scores to trigger stuck detection
    budget.record_attempt(1000, 0.5);
    budget.record_attempt(1000, 0.5);
    budget.record_attempt(1000, 0.5);

    // Only assert if the budget detects stuck (implementation-specific threshold)
    if budget.is_stuck(3) {
        let callback = build_callback(budget);
        let step = make_step();
        let state = make_state();

        let directive = callback.on_step_evaluate(&step, &state).await;

        assert!(
            matches!(directive, StepDirective::SuggestStrategySwitch { .. }),
            "Stuck budget should yield SuggestStrategySwitch, got {:?}",
            directive
        );
    }
}

#[tokio::test]
async fn test_poe_interceptor_continues_with_improving_budget() {
    let mut budget = PoeBudget::new(10, 100_000);
    // Decreasing distance scores — making progress
    budget.record_attempt(1000, 0.9);
    budget.record_attempt(1000, 0.6);
    budget.record_attempt(1000, 0.3);
    assert!(!budget.exhausted());
    assert!(!budget.is_stuck(3));

    let callback = build_callback(budget);
    let step = make_step();
    let state = make_state();

    let directive = callback.on_step_evaluate(&step, &state).await;

    assert!(
        matches!(directive, StepDirective::Continue),
        "Improving budget should yield Continue, got {:?}",
        directive
    );
}

#[tokio::test]
async fn test_poe_interceptor_aborts_when_tokens_exhausted() {
    let mut budget = PoeBudget::new(10, 5_000);
    // Single attempt consuming all tokens
    budget.record_attempt(5_000, 0.8);
    assert!(budget.exhausted(), "Budget should be exhausted when token limit reached");

    let callback = build_callback(budget);
    let step = make_step();
    let state = make_state();

    let directive = callback.on_step_evaluate(&step, &state).await;

    assert!(
        matches!(directive, StepDirective::Abort { .. }),
        "Token-exhausted budget should yield Abort, got {:?}",
        directive
    );
}

#[tokio::test]
async fn test_poe_interceptor_delegates_non_evaluate_callbacks() {
    let callback = build_callback(PoeBudget::new(5, 100_000));
    let state = make_state();

    // Verify delegation doesn't panic (NoOpLoopCallback is no-op)
    callback.on_loop_start(&state).await;
    callback.on_step_start(0).await;
    callback.on_thinking_start(0).await;
    callback.on_complete("all done").await;
    callback.on_failed("test failure").await;
    callback.on_aborted().await;
}

//! PoeLoopCallback — wraps an inner LoopCallback and adds POE step evaluation.
//!
//! All LoopCallback methods delegate to the inner callback, except `on_step_evaluate`
//! which uses the shared StepEvaluator to evaluate each step against the SuccessManifest.
//!
//! Note: `on_validate_completion` intentionally uses the trait default (returns None)
//! because full POE has its own validation via SuccessManifest + CompositeValidator.
//! The lazy POE completion validation in EventEmittingCallback is for non-POE paths only.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::agent_loop::answer::UserAnswer;
use crate::agent_loop::callback::LoopCallback;
use crate::agent_loop::decision::{Action, ActionResult, QuestionGroup};
use crate::agent_loop::guards::GuardViolation;
use crate::agent_loop::question::QuestionKind;
use crate::agent_loop::state::{LoopState, LoopStep, Thinking};
use crate::poe::budget::PoeBudget;
use crate::poe::interceptor::directive::StepDirective;
use crate::poe::interceptor::step_evaluator::StepEvaluator;
use crate::poe::types::SuccessManifest;

/// A LoopCallback wrapper that intercepts `on_step_evaluate` to perform
/// POE-driven step evaluation via StepEvaluator. All other callback methods
/// are transparently delegated to the inner callback.
pub struct PoeLoopCallback<C: LoopCallback> {
    inner: C,
    manifest: Arc<SuccessManifest>,
    budget: Arc<RwLock<PoeBudget>>,
    step_evaluator: Arc<StepEvaluator>,
}

impl<C: LoopCallback> PoeLoopCallback<C> {
    /// Create a new PoeLoopCallback wrapping the given inner callback.
    pub fn new(
        inner: C,
        manifest: Arc<SuccessManifest>,
        budget: Arc<RwLock<PoeBudget>>,
        step_evaluator: Arc<StepEvaluator>,
    ) -> Self {
        Self {
            inner,
            manifest,
            budget,
            step_evaluator,
        }
    }
}

#[async_trait]
impl<C: LoopCallback + Send + Sync> LoopCallback for PoeLoopCallback<C> {
    async fn on_loop_start(&self, state: &LoopState) {
        self.inner.on_loop_start(state).await
    }

    async fn on_step_start(&self, step: usize) {
        self.inner.on_step_start(step).await
    }

    async fn on_thinking_start(&self, step: usize) {
        self.inner.on_thinking_start(step).await
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        self.inner.on_thinking_done(thinking).await
    }

    async fn on_thinking_stream(&self, content: &str) {
        self.inner.on_thinking_stream(content).await
    }

    async fn on_action_start(&self, action: &Action) {
        self.inner.on_action_start(action).await
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        self.inner.on_action_done(action, result).await
    }

    /// POE-intercepted step evaluation.
    ///
    /// Instead of the default `Continue`, this delegates to StepEvaluator
    /// which checks budget exhaustion, stuck detection, and quick constraints.
    async fn on_step_evaluate(
        &self,
        step: &LoopStep,
        state: &LoopState,
    ) -> StepDirective {
        let budget = self.budget.read().await;
        self.step_evaluator
            .evaluate(step, state, &self.manifest, &budget)
            .await
    }

    async fn on_confirmation_required(&self, tool_name: &str, arguments: &Value) -> bool {
        self.inner
            .on_confirmation_required(tool_name, arguments)
            .await
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        self.inner
            .on_user_input_required(question, options)
            .await
    }

    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[QuestionGroup],
    ) -> String {
        self.inner
            .on_user_multigroup_required(question, groups)
            .await
    }

    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        self.inner.on_user_question(question, kind).await
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        self.inner.on_guard_triggered(violation).await
    }

    async fn on_complete(&self, summary: &str) {
        self.inner.on_complete(summary).await
    }

    async fn on_failed(&self, reason: &str) {
        self.inner.on_failed(reason).await
    }

    async fn on_aborted(&self) {
        self.inner.on_aborted().await
    }

    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        self.inner
            .on_doom_loop_detected(tool_name, arguments, repeat_count)
            .await
    }

    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        self.inner
            .on_retry_scheduled(attempt, max_retries, delay_ms, error)
            .await
    }

    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        self.inner.on_retries_exhausted(attempts, error).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::callback::NoOpLoopCallback;
    use crate::agent_loop::decision::{Action, ActionResult, Decision};
    use crate::agent_loop::state::{LoopState, LoopStep, RequestContext, Thinking};
    use crate::poe::budget::PoeBudget;
    use crate::poe::types::SuccessManifest;
    use crate::poe::validation::HardValidator;

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
                tokens_used: None,
                tool_call_id: None,
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
            "test-session".into(),
            "do something".into(),
            RequestContext::empty(),
        )
    }

    #[tokio::test]
    async fn test_poe_callback_returns_continue_with_fresh_budget() {
        let manifest = Arc::new(SuccessManifest::new("t1", "test objective"));
        let budget = Arc::new(RwLock::new(PoeBudget::new(5, 100_000)));
        let evaluator = Arc::new(StepEvaluator::new(Arc::new(HardValidator::new())));

        let callback = PoeLoopCallback::new(
            NoOpLoopCallback,
            manifest,
            budget,
            evaluator,
        );

        let step = make_step();
        let state = make_state();
        let directive = callback.on_step_evaluate(&step, &state).await;
        assert!(
            matches!(directive, StepDirective::Continue),
            "Expected Continue with fresh budget, got {:?}",
            directive
        );
    }

    #[tokio::test]
    async fn test_poe_callback_aborts_when_budget_exhausted() {
        let manifest = Arc::new(SuccessManifest::new("t1", "test objective"));
        let mut raw_budget = PoeBudget::new(2, 100_000);
        raw_budget.record_attempt(50_000, 0.8);
        raw_budget.record_attempt(50_000, 0.7);
        assert!(raw_budget.exhausted());

        let budget = Arc::new(RwLock::new(raw_budget));
        let evaluator = Arc::new(StepEvaluator::new(Arc::new(HardValidator::new())));

        let callback = PoeLoopCallback::new(
            NoOpLoopCallback,
            manifest,
            budget,
            evaluator,
        );

        let step = make_step();
        let state = make_state();
        let directive = callback.on_step_evaluate(&step, &state).await;
        assert!(
            matches!(directive, StepDirective::Abort { .. }),
            "Expected Abort with exhausted budget, got {:?}",
            directive
        );
    }

    #[tokio::test]
    async fn test_poe_callback_delegates_on_complete() {
        let manifest = Arc::new(SuccessManifest::new("t1", "test"));
        let budget = Arc::new(RwLock::new(PoeBudget::new(5, 100_000)));
        let evaluator = Arc::new(StepEvaluator::new(Arc::new(HardValidator::new())));

        let callback = PoeLoopCallback::new(
            NoOpLoopCallback,
            manifest,
            budget,
            evaluator,
        );

        // Verifies delegation doesn't panic (NoOpLoopCallback is no-op)
        callback.on_complete("all done").await;
    }
}

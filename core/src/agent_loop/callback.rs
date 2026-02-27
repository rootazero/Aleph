//! Callback interface for Agent Loop
//!
//! This module defines the callback trait that UI layers implement
//! to receive events from the Agent Loop.

use async_trait::async_trait;
use serde_json::Value;

use super::answer::UserAnswer;
use super::decision::{Action, ActionResult};
use super::guards::GuardViolation;
use super::question::QuestionKind;
use super::state::{LoopState, Thinking};
use crate::poe::StepDirective;

/// Callback interface for Agent Loop events
///
/// UI layers implement this trait to receive real-time updates
/// from the Agent Loop execution.
#[async_trait]
pub trait LoopCallback: Send + Sync {
    /// Called when the loop starts
    async fn on_loop_start(&self, state: &LoopState);

    /// Called when a new step begins
    async fn on_step_start(&self, step: usize);

    /// Called when thinking starts
    async fn on_thinking_start(&self, step: usize);

    /// Called when thinking completes with the result
    async fn on_thinking_done(&self, thinking: &Thinking);

    /// Called when streaming thinking content (optional)
    async fn on_thinking_stream(&self, _content: &str) {
        // Default: no-op
    }

    /// Called when action execution starts
    async fn on_action_start(&self, action: &Action);

    /// Called when action execution completes
    async fn on_action_done(&self, action: &Action, result: &ActionResult);

    /// Called after each step completes (action + result available).
    /// Returns a StepDirective telling AgentLoop what to do next.
    /// Default: Continue (backward compatible — no intervention).
    async fn on_step_evaluate(
        &self,
        _step: &super::state::LoopStep,
        _state: &super::state::LoopState,
    ) -> StepDirective {
        StepDirective::Continue
    }

    /// Called when high-risk operation needs confirmation
    /// Returns true if confirmed, false if cancelled
    async fn on_confirmation_required(&self, tool_name: &str, arguments: &Value) -> bool;

    /// Called when LLM asks for user input
    /// Returns the user's response
    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String;

    /// Called when LLM asks for multi-group user input
    /// Returns the user's response in JSON format (e.g., "{\"group1\":\"value1\",\"group2\":\"value2\"}")
    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[super::decision::QuestionGroup],
    ) -> String;

    /// Called when LLM asks for rich user input with structured question type
    /// Returns the user's structured response
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        // Default implementation: convert to legacy format for backward compatibility
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                let response = self.on_user_input_required(question, None).await;
                let confirmed = response.to_lowercase() == "yes"
                    || response.to_lowercase() == "y"
                    || response == "true"
                    || (response.is_empty() && *default);
                UserAnswer::Confirmation { confirmed }
            }
            QuestionKind::SingleChoice { choices, .. } => {
                let options: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
                let response = self.on_user_input_required(question, Some(&options)).await;
                let selected_index = choices
                    .iter()
                    .position(|c| c.label == response)
                    .unwrap_or(0);
                UserAnswer::SingleChoice {
                    selected_index,
                    selected_label: response,
                }
            }
            QuestionKind::MultiChoice { choices, .. } => {
                let options: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
                let response = self.on_user_input_required(question, Some(&options)).await;
                let selections: Vec<&str> = response.split(',').map(|s| s.trim()).collect();
                let mut indices = Vec::new();
                let mut labels = Vec::new();
                for sel in selections {
                    if let Some(idx) = choices.iter().position(|c| c.label == sel) {
                        indices.push(idx);
                        labels.push(sel.to_string());
                    }
                }
                UserAnswer::MultiChoice {
                    selected_indices: indices,
                    selected_labels: labels,
                }
            }
            QuestionKind::TextInput { .. } => {
                let response = self.on_user_input_required(question, None).await;
                UserAnswer::TextInput { text: response }
            }
        }
    }

    /// Called when a guard is triggered
    async fn on_guard_triggered(&self, violation: &GuardViolation);

    /// Called when task completes successfully
    async fn on_complete(&self, summary: &str);

    /// Called when task fails
    async fn on_failed(&self, reason: &str);

    /// Called when loop is aborted by user
    async fn on_aborted(&self) {
        // Default: no-op
    }

    /// Called when a doom loop is detected
    ///
    /// This is a more severe condition than a guard violation, indicating
    /// that the agent is making the exact same call repeatedly.
    /// Returns true if the user wants to continue anyway (will reset detection).
    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        // Default: don't continue
        let _ = (tool_name, arguments, repeat_count);
        false
    }

    /// Called when a retry is scheduled for a failed operation
    ///
    /// This informs the UI that a retry will be attempted after the specified delay.
    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        // Default: no-op
        let _ = (attempt, max_retries, delay_ms, error);
    }

    /// Called when a retryable error occurs but retries are exhausted
    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        // Default: no-op
        let _ = (attempts, error);
    }
}

/// Blanket implementation for references to LoopCallback
#[async_trait]
impl<T: LoopCallback + ?Sized> LoopCallback for &T {
    async fn on_loop_start(&self, state: &LoopState) {
        (*self).on_loop_start(state).await
    }
    async fn on_step_start(&self, step: usize) {
        (*self).on_step_start(step).await
    }
    async fn on_thinking_start(&self, step: usize) {
        (*self).on_thinking_start(step).await
    }
    async fn on_thinking_done(&self, thinking: &Thinking) {
        (*self).on_thinking_done(thinking).await
    }
    async fn on_thinking_stream(&self, content: &str) {
        (*self).on_thinking_stream(content).await
    }
    async fn on_action_start(&self, action: &Action) {
        (*self).on_action_start(action).await
    }
    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        (*self).on_action_done(action, result).await
    }
    async fn on_step_evaluate(
        &self,
        step: &super::state::LoopStep,
        state: &super::state::LoopState,
    ) -> StepDirective {
        (*self).on_step_evaluate(step, state).await
    }
    async fn on_confirmation_required(&self, tool_name: &str, arguments: &Value) -> bool {
        (*self).on_confirmation_required(tool_name, arguments).await
    }
    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        (*self).on_user_input_required(question, options).await
    }
    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[super::decision::QuestionGroup],
    ) -> String {
        (*self)
            .on_user_multigroup_required(question, groups)
            .await
    }
    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        (*self).on_user_question(question, kind).await
    }
    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        (*self).on_guard_triggered(violation).await
    }
    async fn on_complete(&self, summary: &str) {
        (*self).on_complete(summary).await
    }
    async fn on_failed(&self, reason: &str) {
        (*self).on_failed(reason).await
    }
    async fn on_aborted(&self) {
        (*self).on_aborted().await
    }
    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        (*self)
            .on_doom_loop_detected(tool_name, arguments, repeat_count)
            .await
    }
    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        (*self)
            .on_retry_scheduled(attempt, max_retries, delay_ms, error)
            .await
    }
    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        (*self).on_retries_exhausted(attempts, error).await
    }
}

/// No-op callback implementation for testing Agent Loop
///
/// This is distinct from `dispatcher::NoOpExecutionCallback` which implements
/// `ExecutionCallback` for task execution callbacks.
pub struct NoOpLoopCallback;

#[async_trait]
impl LoopCallback for NoOpLoopCallback {
    async fn on_loop_start(&self, _state: &LoopState) {}
    async fn on_step_start(&self, _step: usize) {}
    async fn on_thinking_start(&self, _step: usize) {}
    async fn on_thinking_done(&self, _thinking: &Thinking) {}
    async fn on_action_start(&self, _action: &Action) {}
    async fn on_action_done(&self, _action: &Action, _result: &ActionResult) {}

    async fn on_confirmation_required(&self, _tool_name: &str, _arguments: &Value) -> bool {
        true // Auto-confirm in tests
    }

    async fn on_user_input_required(
        &self,
        _question: &str,
        _options: Option<&[String]>,
    ) -> String {
        "ok".to_string() // Auto-respond in tests
    }

    async fn on_user_multigroup_required(
        &self,
        _question: &str,
        _groups: &[super::decision::QuestionGroup],
    ) -> String {
        // Auto-respond with first option for each group in JSON format
        "{\"default\":\"ok\"}".to_string()
    }

    async fn on_user_question(&self, _question: &str, kind: &QuestionKind) -> UserAnswer {
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            QuestionKind::SingleChoice {
                choices,
                default_index,
            } => {
                let idx = default_index.unwrap_or(0);
                let label = choices.get(idx).map(|c| c.label.clone()).unwrap_or_default();
                UserAnswer::SingleChoice {
                    selected_index: idx,
                    selected_label: label,
                }
            }
            QuestionKind::MultiChoice { choices, .. } => {
                let label = choices.first().map(|c| c.label.clone()).unwrap_or_default();
                UserAnswer::MultiChoice {
                    selected_indices: vec![0],
                    selected_labels: vec![label],
                }
            }
            QuestionKind::TextInput { .. } => UserAnswer::TextInput {
                text: "ok".to_string(),
            },
        }
    }

    async fn on_guard_triggered(&self, _violation: &GuardViolation) {}
    async fn on_complete(&self, _summary: &str) {}
    async fn on_failed(&self, _reason: &str) {}
}

/// Logging callback for debugging
pub struct LoggingCallback {
    prefix: String,
}

impl LoggingCallback {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }
}

#[async_trait]
impl LoopCallback for LoggingCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        tracing::info!(
            "{} Loop started: session={}, request={}",
            self.prefix,
            state.session_id,
            state.original_request
        );
    }

    async fn on_step_start(&self, step: usize) {
        tracing::info!("{} Step {} started", self.prefix, step);
    }

    async fn on_thinking_start(&self, step: usize) {
        tracing::debug!("{} Thinking started for step {}", self.prefix, step);
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        tracing::info!(
            "{} Thinking done: decision={:?}",
            self.prefix,
            thinking.decision.decision_type()
        );
    }

    async fn on_action_start(&self, action: &Action) {
        tracing::info!("{} Action started: {}", self.prefix, action.action_type());
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        tracing::info!(
            "{} Action done: {} -> success={}",
            self.prefix,
            action.action_type(),
            result.is_success()
        );
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        tracing::warn!(
            "{} Confirmation required for tool: {} (auto-confirming)",
            self.prefix,
            tool_name
        );
        true
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        _options: Option<&[String]>,
    ) -> String {
        tracing::warn!(
            "{} User input required: {} (auto-responding)",
            self.prefix,
            question
        );
        "continue".to_string()
    }

    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[super::decision::QuestionGroup],
    ) -> String {
        tracing::warn!(
            "{} Multi-group input required: {} (groups: {}) (auto-responding)",
            self.prefix,
            question,
            groups.len()
        );
        "{\"default\":\"ok\"}".to_string()
    }

    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        tracing::warn!(
            "{} Rich user input required: {} (type: {:?}) (auto-responding)",
            self.prefix,
            question,
            std::mem::discriminant(kind)
        );
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            _ => UserAnswer::TextInput {
                text: "continue".to_string(),
            },
        }
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        tracing::error!("{} Guard triggered: {}", self.prefix, violation.description());
    }

    async fn on_complete(&self, summary: &str) {
        tracing::info!("{} Loop completed: {}", self.prefix, summary);
    }

    async fn on_failed(&self, reason: &str) {
        tracing::error!("{} Loop failed: {}", self.prefix, reason);
    }

    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        _arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        tracing::error!(
            "{} Doom loop detected: {} called {} times with identical arguments",
            self.prefix,
            tool_name,
            repeat_count
        );
        false // Don't continue by default
    }

    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        tracing::warn!(
            "{} Retry scheduled: attempt {}/{}, delay {}ms, error: {}",
            self.prefix,
            attempt,
            max_retries,
            delay_ms,
            error
        );
    }

    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        tracing::error!(
            "{} Retries exhausted after {} attempts: {}",
            self.prefix,
            attempts,
            error
        );
    }
}

/// Callback that collects events for testing/inspection
#[derive(Default)]
pub struct CollectingCallback {
    events: std::sync::Mutex<Vec<LoopEvent>>,
}

/// Event types for collecting callback
#[derive(Debug, Clone)]
pub enum LoopEvent {
    LoopStart { session_id: String },
    StepStart { step: usize },
    ThinkingStart { step: usize },
    ThinkingDone { decision_type: String },
    ActionStart { action_type: String },
    ActionDone { action_type: String, success: bool },
    StepEvaluate { step: usize },
    ConfirmationRequired { tool_name: String },
    UserInputRequired { question: String },
    UserMultigroupRequired { question: String, group_count: usize },
    UserQuestionRequired { question: String, kind_type: String },
    GuardTriggered { description: String },
    Complete { summary: String },
    Failed { reason: String },
    DoomLoopDetected { tool_name: String, repeat_count: usize },
    RetryScheduled { attempt: u32, max_retries: u32, delay_ms: u64, error: String },
    RetriesExhausted { attempts: u32, error: String },
}

impl CollectingCallback {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<LoopEvent> {
        self.events.lock().unwrap().clone()
    }

    fn push(&self, event: LoopEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[async_trait]
impl LoopCallback for CollectingCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        self.push(LoopEvent::LoopStart {
            session_id: state.session_id.clone(),
        });
    }

    async fn on_step_start(&self, step: usize) {
        self.push(LoopEvent::StepStart { step });
    }

    async fn on_thinking_start(&self, step: usize) {
        self.push(LoopEvent::ThinkingStart { step });
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        self.push(LoopEvent::ThinkingDone {
            decision_type: thinking.decision.decision_type().to_string(),
        });
    }

    async fn on_action_start(&self, action: &Action) {
        self.push(LoopEvent::ActionStart {
            action_type: action.action_type(),
        });
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        self.push(LoopEvent::ActionDone {
            action_type: action.action_type(),
            success: result.is_success(),
        });
    }

    async fn on_step_evaluate(
        &self,
        step: &super::state::LoopStep,
        _state: &super::state::LoopState,
    ) -> StepDirective {
        self.push(LoopEvent::StepEvaluate { step: step.step_id });
        StepDirective::Continue
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        self.push(LoopEvent::ConfirmationRequired {
            tool_name: tool_name.to_string(),
        });
        true
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        _options: Option<&[String]>,
    ) -> String {
        self.push(LoopEvent::UserInputRequired {
            question: question.to_string(),
        });
        "test_response".to_string()
    }

    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[super::decision::QuestionGroup],
    ) -> String {
        self.push(LoopEvent::UserMultigroupRequired {
            question: question.to_string(),
            group_count: groups.len(),
        });
        "{\"default\":\"ok\"}".to_string()
    }

    async fn on_user_question(&self, question: &str, kind: &QuestionKind) -> UserAnswer {
        self.push(LoopEvent::UserQuestionRequired {
            question: question.to_string(),
            kind_type: format!("{:?}", std::mem::discriminant(kind)),
        });
        match kind {
            QuestionKind::Confirmation { default, .. } => {
                UserAnswer::Confirmation { confirmed: *default }
            }
            QuestionKind::SingleChoice {
                choices,
                default_index,
            } => {
                let idx = default_index.unwrap_or(0);
                UserAnswer::SingleChoice {
                    selected_index: idx,
                    selected_label: choices.get(idx).map(|c| c.label.clone()).unwrap_or_default(),
                }
            }
            QuestionKind::MultiChoice { .. } => UserAnswer::MultiChoice {
                selected_indices: vec![],
                selected_labels: vec![],
            },
            QuestionKind::TextInput { .. } => UserAnswer::TextInput {
                text: "test_response".to_string(),
            },
        }
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        self.push(LoopEvent::GuardTriggered {
            description: violation.description(),
        });
    }

    async fn on_complete(&self, summary: &str) {
        self.push(LoopEvent::Complete {
            summary: summary.to_string(),
        });
    }

    async fn on_failed(&self, reason: &str) {
        self.push(LoopEvent::Failed {
            reason: reason.to_string(),
        });
    }

    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        _arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        self.push(LoopEvent::DoomLoopDetected {
            tool_name: tool_name.to_string(),
            repeat_count,
        });
        false // Don't continue
    }

    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        self.push(LoopEvent::RetryScheduled {
            attempt,
            max_retries,
            delay_ms,
            error: error.to_string(),
        });
    }

    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        self.push(LoopEvent::RetriesExhausted {
            attempts,
            error: error.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collecting_callback() {
        let callback = CollectingCallback::new();

        let state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            super::super::state::RequestContext::empty(),
        );

        callback.on_loop_start(&state).await;
        callback.on_step_start(0).await;
        callback.on_complete("done").await;

        let events = callback.events();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], LoopEvent::LoopStart { .. }));
        assert!(matches!(events[1], LoopEvent::StepStart { step: 0 }));
        assert!(matches!(events[2], LoopEvent::Complete { .. }));
    }

    #[tokio::test]
    async fn test_on_step_evaluate_default_returns_continue() {
        use super::super::decision::{Action, ActionResult, Decision};
        use super::super::state::{LoopStep, Thinking};

        let callback = NoOpLoopCallback;
        let state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            super::super::state::RequestContext::empty(),
        );
        let step = LoopStep {
            step_id: 0,
            observation_summary: String::new(),
            thinking: Thinking {
                reasoning: None,
                decision: Decision::Complete {
                    summary: "done".to_string(),
                },
                structured: None,
            },
            action: Action::Completion {
                summary: "done".to_string(),
            },
            result: ActionResult::Completed,
            tokens_used: 0,
            duration_ms: 0,
        };
        let directive = callback.on_step_evaluate(&step, &state).await;
        assert!(matches!(directive, StepDirective::Continue));
    }

    #[tokio::test]
    async fn test_collecting_callback_user_question() {
        use super::super::answer::UserAnswer;
        use super::super::question::{ChoiceOption, QuestionKind};

        let callback = CollectingCallback::new();

        let kind = QuestionKind::SingleChoice {
            choices: vec![ChoiceOption::new("A"), ChoiceOption::new("B")],
            default_index: Some(1),
        };

        let answer = callback.on_user_question("Pick one", &kind).await;

        assert!(matches!(
            answer,
            UserAnswer::SingleChoice {
                selected_index: 1,
                ..
            }
        ));

        let events = callback.events();
        assert!(events
            .iter()
            .any(|e| matches!(e, LoopEvent::UserQuestionRequired { .. })));
    }
}

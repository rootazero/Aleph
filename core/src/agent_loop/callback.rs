//! Callback interface for Agent Loop
//!
//! This module defines the callback trait that UI layers implement
//! to receive events from the Agent Loop.

use async_trait::async_trait;
use serde_json::Value;

use super::decision::{Action, ActionResult};
use super::guards::GuardViolation;
use super::state::{LoopState, Thinking};

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
}

/// No-op callback implementation for testing
pub struct NoOpCallback;

#[async_trait]
impl LoopCallback for NoOpCallback {
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

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        tracing::error!("{} Guard triggered: {}", self.prefix, violation.description());
    }

    async fn on_complete(&self, summary: &str) {
        tracing::info!("{} Loop completed: {}", self.prefix, summary);
    }

    async fn on_failed(&self, reason: &str) {
        tracing::error!("{} Loop failed: {}", self.prefix, reason);
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
    ConfirmationRequired { tool_name: String },
    UserInputRequired { question: String },
    GuardTriggered { description: String },
    Complete { summary: String },
    Failed { reason: String },
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
}

//! Agent Loop FFI Adapter
//!
//! This module provides the bridge between the new AgentLoop architecture
//! and the existing FFI event handler interface.
//!
//! # Architecture
//!
//! ```text
//! Swift UI ← AetherEventHandler ← FfiLoopCallback ← AgentLoop
//! ```
//!
//! The `FfiLoopCallback` translates AgentLoop events into FFI callbacks
//! that the Swift UI layer can understand and display.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::agent_loop::{
    callback::LoopCallback, guards::GuardViolation, Action, ActionResult, LoopState, Thinking,
};
use crate::ffi::AetherEventHandler;

/// FFI-compatible callback adapter for AgentLoop
///
/// This adapter translates AgentLoop callback events into
/// AetherEventHandler calls that the UI layer understands.
pub struct FfiLoopCallback {
    /// The underlying FFI event handler
    handler: Arc<dyn AetherEventHandler>,
    /// Accumulated response text for streaming
    response_buffer: RwLock<String>,
    /// Whether streaming has started
    streaming_started: RwLock<bool>,
}

impl FfiLoopCallback {
    /// Create a new FFI callback adapter
    pub fn new(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self {
            handler,
            response_buffer: RwLock::new(String::new()),
            streaming_started: RwLock::new(false),
        }
    }

    /// Get the accumulated response
    pub async fn get_response(&self) -> String {
        self.response_buffer.read().await.clone()
    }

    /// Append text to response and stream it
    async fn stream_text(&self, text: &str) {
        let mut started = self.streaming_started.write().await;
        if !*started {
            // Use on_thinking to signal start (no dedicated on_response_start)
            // The handler is already called on_thinking in on_loop_start
            *started = true;
        }
        drop(started);

        let mut buffer = self.response_buffer.write().await;
        buffer.push_str(text);

        // Stream the chunk to UI using on_stream_chunk
        self.handler.on_stream_chunk(text.to_string());
    }

    /// Finalize the response
    async fn finalize_response(&self) {
        let buffer = self.response_buffer.read().await;
        let started = self.streaming_started.read().await;

        if *started {
            // Use on_complete to signal completion
            self.handler.on_complete(buffer.clone());
        }
    }
}

#[async_trait]
impl LoopCallback for FfiLoopCallback {
    async fn on_loop_start(&self, state: &LoopState) {
        debug!(
            session_id = %state.session_id,
            request = %state.original_request,
            "AgentLoop started"
        );
        self.handler.on_thinking();
    }

    async fn on_step_start(&self, step: usize) {
        info!(step = step, "AgentLoop step started");
        // Notify UI about step progress (step is 0-indexed, display as 1-indexed)
        if step > 0 {
            // After first step, show iteration progress
            self.stream_text(&format!("\n--- Step {} ---\n", step + 1)).await;
        }
    }

    async fn on_thinking_start(&self, step: usize) {
        debug!(step = step, "Thinking started");
        // UI shows thinking indicator (already set by on_loop_start)
    }

    async fn on_thinking_done(&self, thinking: &Thinking) {
        debug!(
            decision_type = thinking.decision.decision_type(),
            "Thinking completed"
        );

        // If there's reasoning, we could optionally stream it
        if let Some(ref reasoning) = thinking.reasoning {
            // For debugging, log reasoning
            debug!(reasoning = %reasoning, "LLM reasoning");
        }
    }

    async fn on_thinking_stream(&self, content: &str) {
        // Stream thinking content (optional, for verbose mode)
        debug!(content_len = content.len(), "Thinking stream chunk");
    }

    async fn on_action_start(&self, action: &Action) {
        info!(action_type = %action.action_type(), "Action started");

        match action {
            Action::ToolCall { tool_name, .. } => {
                // Notify UI about tool execution start
                info!(tool = %tool_name, "Executing tool");
                self.handler.on_tool_start(tool_name.clone());
            }
            Action::Completion { summary } => {
                // Stream the completion summary as response
                self.stream_text(summary).await;
            }
            Action::UserInteraction { question, .. } => {
                // This will be handled by on_user_input_required
                debug!(question = %question, "User interaction requested");
            }
            Action::Failure { reason } => {
                // Stream the failure reason
                self.stream_text(&format!("Error: {}", reason)).await;
            }
        }
    }

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        info!(
            action_type = %action.action_type(),
            success = result.is_success(),
            "Action completed"
        );

        // Notify UI about tool execution results
        if let Action::ToolCall { tool_name, .. } = action {
            match result {
                ActionResult::ToolSuccess { output, duration_ms } => {
                    info!(
                        tool = %tool_name,
                        duration_ms = duration_ms,
                        output_size = output.to_string().len(),
                        "Tool execution successful"
                    );
                    // Send tool result to UI (truncate for display)
                    let output_str = output.to_string();
                    let display_output = if output_str.len() > 500 {
                        format!("{}...", &output_str[..500])
                    } else {
                        output_str
                    };
                    self.handler.on_tool_result(tool_name.clone(), display_output);
                }
                ActionResult::ToolError { error, .. } => {
                    warn!(
                        tool = %tool_name,
                        error = %error,
                        "Tool execution failed"
                    );
                    self.handler.on_tool_result(tool_name.clone(), format!("Error: {}", error));
                }
                _ => {}
            }
        }
    }

    async fn on_confirmation_required(&self, tool_name: &str, _arguments: &Value) -> bool {
        info!(
            tool = %tool_name,
            "Confirmation required for tool execution"
        );

        // For now, auto-confirm. In the future, this should prompt the user
        // through the FFI layer using a dedicated confirmation callback
        warn!("Auto-confirming tool execution (confirmation UI not implemented)");
        true
    }

    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        info!(
            question = %question,
            has_options = options.is_some(),
            "User input required"
        );

        // Stream the question to the user
        self.stream_text(question).await;

        // For now, return a placeholder. In the future, this should
        // block until user provides input through the FFI layer
        warn!("Returning placeholder user response (interactive input not implemented)");
        "ok".to_string()
    }

    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        warn!(
            violation = ?violation,
            "Guard triggered"
        );

        // Notify UI about the guard violation
        let message = format!("Limit reached: {}", violation.description());
        self.stream_text(&message).await;
    }

    async fn on_complete(&self, summary: &str) {
        info!(summary_len = summary.len(), "AgentLoop completed");

        // Ensure the summary is in the response
        let buffer = self.response_buffer.read().await;
        if !buffer.contains(summary) {
            drop(buffer);
            self.stream_text(summary).await;
        }

        // Finalize the response
        self.finalize_response().await;
    }

    async fn on_failed(&self, reason: &str) {
        warn!(reason = %reason, "AgentLoop failed");

        // Stream the error
        self.stream_text(&format!("\n\nError: {}", reason)).await;

        // Call error handler
        self.handler.on_error(reason.to_string());
    }

    async fn on_aborted(&self) {
        info!("AgentLoop aborted");
        self.handler.on_error("Operation cancelled".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mock event handler for testing
    struct MockEventHandler {
        events: Mutex<Vec<String>>,
    }

    impl MockEventHandler {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl AetherEventHandler for MockEventHandler {
        fn on_thinking(&self) {
            self.events.lock().unwrap().push("thinking".to_string());
        }

        fn on_tool_start(&self, tool_name: String) {
            self.events.lock().unwrap().push(format!("tool_start:{}", tool_name));
        }

        fn on_tool_result(&self, _tool_name: String, _result: String) {}

        fn on_stream_chunk(&self, chunk: String) {
            self.events.lock().unwrap().push(format!("chunk:{}", chunk));
        }

        fn on_complete(&self, response: String) {
            self.events.lock().unwrap().push(format!("complete:{}", response.len()));
        }

        fn on_error(&self, error: String) {
            self.events.lock().unwrap().push(format!("error:{}", error));
        }

        fn on_memory_stored(&self) {}

        fn on_agent_mode_detected(&self, _task: crate::intent::ExecutableTaskFFI) {}

        fn on_tools_changed(&self, _tool_count: u32) {}

        fn on_mcp_startup_complete(&self, _report: crate::event_handler::McpStartupReportFFI) {}

        fn on_runtime_updates_available(&self, _updates: Vec<crate::ffi::RuntimeUpdateInfo>) {}

        fn on_session_started(&self, _session_id: String) {}

        fn on_tool_call_started(&self, _call_id: String, _tool_name: String) {}

        fn on_tool_call_completed(&self, _call_id: String, _output: String) {}

        fn on_tool_call_failed(&self, _call_id: String, _error: String, _is_retryable: bool) {}

        fn on_loop_progress(&self, _session_id: String, _iteration: u32, _status: String) {}

        fn on_plan_created(&self, _session_id: String, _steps: Vec<String>) {}

        fn on_session_completed(&self, _session_id: String, _summary: String) {}

        fn on_subagent_started(&self, _parent_session_id: String, _child_session_id: String, _agent_id: String) {}

        fn on_subagent_completed(&self, _child_session_id: String, _success: bool, _summary: String) {}

        fn on_plan_confirmation_required(&self, _plan_id: String, _plan: crate::dispatcher::DagTaskPlan) {}
    }

    #[tokio::test]
    async fn test_callback_adapter_streaming() {
        let handler = Arc::new(MockEventHandler::new());
        let callback = FfiLoopCallback::new(handler.clone());

        // Simulate streaming
        callback.stream_text("Hello, ").await;
        callback.stream_text("world!").await;
        callback.finalize_response().await;

        let events = handler.events();
        assert!(events.contains(&"chunk:Hello, ".to_string()));
        assert!(events.contains(&"chunk:world!".to_string()));

        let response = callback.get_response().await;
        assert_eq!(response, "Hello, world!");
    }

    #[tokio::test]
    async fn test_callback_adapter_completion() {
        let handler = Arc::new(MockEventHandler::new());
        let callback = FfiLoopCallback::new(handler.clone());

        // Simulate completion
        let action = Action::Completion {
            summary: "Task completed successfully".to_string(),
        };
        callback.on_action_start(&action).await;
        callback.on_complete("Task completed successfully").await;

        let events = handler.events();
        assert!(events.iter().any(|e| e.starts_with("complete:")));
    }
}

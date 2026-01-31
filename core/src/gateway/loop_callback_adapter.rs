//! LoopCallback Adapter for Gateway
//!
//! Bridges the AgentLoop callback system with the Gateway's StreamEvent emission.
//! Implements the `LoopCallback` trait and translates loop events to WebSocket events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::agent_loop::callback::LoopCallback;
use crate::agent_loop::decision::{Action, ActionResult, QuestionGroup};
use crate::agent_loop::guards::GuardViolation;
use crate::agent_loop::state::{LoopState, Thinking};

use super::event_emitter::{EventEmitter, StreamEvent, ToolResult};

/// Adapter that implements LoopCallback and emits StreamEvents
///
/// This adapter bridges the AgentLoop's callback interface with the Gateway's
/// WebSocket event system, enabling real-time streaming of agent execution
/// to connected clients.
pub struct EventEmittingCallback<E: EventEmitter> {
    /// The event emitter for sending events to clients
    emitter: Arc<E>,
    /// Current run ID
    run_id: String,
    /// Sequence counter for events
    seq_counter: AtomicU64,
    /// Chunk counter for response streaming
    chunk_counter: AtomicU64,
    /// Map from tool_name to tool_id for active tools
    tool_id_map: Mutex<HashMap<String, String>>,
    /// Timestamps for tool execution tracking
    tool_start_times: Mutex<HashMap<String, Instant>>,
    /// Channel for user response (for blocking on_user_input_required)
    #[allow(dead_code)]
    user_response_rx: Option<tokio::sync::oneshot::Receiver<String>>,
    /// Channel sender for sending user questions to UI
    user_question_tx: Option<tokio::sync::mpsc::Sender<UserQuestion>>,
}

/// User question sent from callback to UI
#[derive(Debug)]
pub struct UserQuestion {
    pub question: String,
    pub options: Option<Vec<String>>,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}

impl<E: EventEmitter> EventEmittingCallback<E> {
    /// Create a new callback adapter
    pub fn new(emitter: Arc<E>, run_id: String) -> Self {
        Self {
            emitter,
            run_id,
            seq_counter: AtomicU64::new(0),
            chunk_counter: AtomicU64::new(0),
            tool_id_map: Mutex::new(HashMap::new()),
            tool_start_times: Mutex::new(HashMap::new()),
            user_response_rx: None,
            user_question_tx: None,
        }
    }

    /// Create with user interaction channel
    ///
    /// This is used when the callback needs to support interactive user input.
    pub fn with_user_channel(
        emitter: Arc<E>,
        run_id: String,
        user_question_tx: tokio::sync::mpsc::Sender<UserQuestion>,
    ) -> Self {
        Self {
            emitter,
            run_id,
            seq_counter: AtomicU64::new(0),
            chunk_counter: AtomicU64::new(0),
            tool_id_map: Mutex::new(HashMap::new()),
            tool_start_times: Mutex::new(HashMap::new()),
            user_response_rx: None,
            user_question_tx: Some(user_question_tx),
        }
    }

    /// Get next sequence number
    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get next chunk index
    fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst) as u32
    }

    /// Generate a unique tool ID
    fn generate_tool_id(&self, tool_name: &str) -> String {
        format!("{}_{}", tool_name, uuid::Uuid::new_v4())
    }
}

#[async_trait]
impl<E: EventEmitter + Send + Sync + 'static> LoopCallback for EventEmittingCallback<E> {
    /// Called when the loop starts
    async fn on_loop_start(&self, state: &LoopState) {
        tracing::debug!(
            run_id = %self.run_id,
            session_id = %state.session_id,
            "Loop started"
        );
        // RunAccepted is already emitted by ExecutionEngine, no need to duplicate
    }

    /// Called when a new step begins
    async fn on_step_start(&self, step: usize) {
        tracing::debug!(run_id = %self.run_id, step = step, "Step started");
    }

    /// Called when thinking starts
    async fn on_thinking_start(&self, step: usize) {
        let _ = self
            .emitter
            .emit(StreamEvent::Reasoning {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                content: format!("Thinking (step {})...", step + 1),
                is_complete: false,
            })
            .await;
    }

    /// Called when thinking completes
    async fn on_thinking_done(&self, thinking: &Thinking) {
        // Emit reasoning content if available
        if let Some(ref reasoning) = thinking.reasoning {
            let _ = self
                .emitter
                .emit(StreamEvent::Reasoning {
                    run_id: self.run_id.clone(),
                    seq: self.next_seq(),
                    content: reasoning.clone(),
                    is_complete: true,
                })
                .await;
        } else {
            // Still emit completion signal
            let _ = self
                .emitter
                .emit(StreamEvent::Reasoning {
                    run_id: self.run_id.clone(),
                    seq: self.next_seq(),
                    content: String::new(),
                    is_complete: true,
                })
                .await;
        }
    }

    /// Called when streaming thinking content
    async fn on_thinking_stream(&self, content: &str) {
        let _ = self
            .emitter
            .emit(StreamEvent::Reasoning {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                content: content.to_string(),
                is_complete: false,
            })
            .await;
    }

    /// Called when action execution starts
    async fn on_action_start(&self, action: &Action) {
        if let Action::ToolCall {
            tool_name,
            arguments,
        } = action
        {
            let tool_id = self.generate_tool_id(tool_name);

            // Store mapping and start time
            {
                let mut map = self.tool_id_map.lock().await;
                map.insert(tool_name.clone(), tool_id.clone());
            }
            {
                let mut times = self.tool_start_times.lock().await;
                times.insert(tool_id.clone(), Instant::now());
            }

            let _ = self
                .emitter
                .emit(StreamEvent::ToolStart {
                    run_id: self.run_id.clone(),
                    seq: self.next_seq(),
                    tool_name: tool_name.clone(),
                    tool_id,
                    params: arguments.clone(),
                })
                .await;
        }
    }

    /// Called when action execution completes
    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        if let Action::ToolCall { tool_name, .. } = action {
            // Get tool ID and duration
            let tool_id = {
                let map = self.tool_id_map.lock().await;
                map.get(tool_name).cloned().unwrap_or_default()
            };

            let duration_ms = {
                let times = self.tool_start_times.lock().await;
                times
                    .get(&tool_id)
                    .map(|start| start.elapsed().as_millis() as u64)
                    .unwrap_or(0)
            };

            // Convert ActionResult to ToolResult
            let tool_result = match result {
                ActionResult::ToolSuccess { output, .. } => ToolResult::success(output.to_string()),
                ActionResult::ToolError { error, .. } => ToolResult::error(error),
                ActionResult::UserResponse { response } => ToolResult::success(response),
                ActionResult::UserResponseRich { response } => ToolResult::success(response.to_llm_feedback()),
                ActionResult::Completed => ToolResult::success("Completed"),
                ActionResult::Failed => ToolResult::error("Failed"),
            };

            let _ = self
                .emitter
                .emit(StreamEvent::ToolEnd {
                    run_id: self.run_id.clone(),
                    seq: self.next_seq(),
                    tool_id,
                    result: tool_result,
                    duration_ms,
                })
                .await;
        }
    }

    /// Called when confirmation is required for high-risk operations
    async fn on_confirmation_required(&self, tool_name: &str, arguments: &Value) -> bool {
        tracing::warn!(
            run_id = %self.run_id,
            tool = %tool_name,
            "Confirmation required, auto-approving"
        );
        // For now, auto-approve (TODO: implement UI confirmation via Gateway)
        let _ = (tool_name, arguments);
        true
    }

    /// Called when user input is required
    async fn on_user_input_required(
        &self,
        question: &str,
        options: Option<&[String]>,
    ) -> String {
        // Emit AskUser event
        let _ = self
            .emitter
            .emit(StreamEvent::AskUser {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                question: question.to_string(),
                options: options.map(|o| o.to_vec()).unwrap_or_default(),
            })
            .await;

        // If we have a channel to send questions, use it
        if let Some(ref tx) = self.user_question_tx {
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let user_question = UserQuestion {
                question: question.to_string(),
                options: options.map(|o| o.to_vec()),
                response_tx,
            };

            if tx.send(user_question).await.is_ok() {
                // Wait for user response
                if let Ok(response) = response_rx.await {
                    return response;
                }
            }
        }

        // Default response if no channel or error
        tracing::warn!(
            run_id = %self.run_id,
            question = %question,
            "User input required but no response channel, using default"
        );
        "continue".to_string()
    }

    /// Called when multi-group user input is required
    async fn on_user_multigroup_required(
        &self,
        question: &str,
        groups: &[QuestionGroup],
    ) -> String {
        tracing::debug!(
            run_id = %self.run_id,
            question = %question,
            groups = groups.len(),
            "Multi-group input required"
        );
        // TODO: Implement multi-group question via Gateway
        "{\"default\":\"ok\"}".to_string()
    }

    /// Called when a guard is triggered
    async fn on_guard_triggered(&self, violation: &GuardViolation) {
        let error_msg = violation.description();
        let _ = self
            .emitter
            .emit(StreamEvent::RunError {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                error: error_msg,
                error_code: Some("GUARD_TRIGGERED".to_string()),
            })
            .await;
    }

    /// Called when task completes successfully
    async fn on_complete(&self, summary: &str) {
        // Emit final response chunk with the summary
        let _ = self
            .emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                content: summary.to_string(),
                chunk_index: self.next_chunk(),
                is_final: true,
            })
            .await;
    }

    /// Called when task fails
    async fn on_failed(&self, reason: &str) {
        let _ = self
            .emitter
            .emit(StreamEvent::RunError {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                error: reason.to_string(),
                error_code: Some("EXECUTION_FAILED".to_string()),
            })
            .await;
    }

    /// Called when loop is aborted by user
    async fn on_aborted(&self) {
        let _ = self
            .emitter
            .emit(StreamEvent::RunError {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                error: "Aborted by user".to_string(),
                error_code: Some("USER_ABORTED".to_string()),
            })
            .await;
    }

    /// Called when doom loop is detected
    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        _arguments: &Value,
        repeat_count: usize,
    ) -> bool {
        tracing::warn!(
            run_id = %self.run_id,
            tool = %tool_name,
            repeat_count = repeat_count,
            "Doom loop detected"
        );
        // Don't continue by default
        false
    }

    /// Called when retry is scheduled
    async fn on_retry_scheduled(&self, attempt: u32, max_retries: u32, delay_ms: u64, error: &str) {
        tracing::debug!(
            run_id = %self.run_id,
            attempt = attempt,
            max_retries = max_retries,
            delay_ms = delay_ms,
            error = %error,
            "Retry scheduled"
        );
    }

    /// Called when retries are exhausted
    async fn on_retries_exhausted(&self, attempts: u32, error: &str) {
        let _ = self
            .emitter
            .emit(StreamEvent::RunError {
                run_id: self.run_id.clone(),
                seq: self.next_seq(),
                error: format!("Retries exhausted after {} attempts: {}", attempts, error),
                error_code: Some("RETRIES_EXHAUSTED".to_string()),
            })
            .await;
    }
}

/// Response chunk emitter for streaming text output
///
/// This helper is used when you need to emit multiple response chunks
/// for a single response (e.g., streaming LLM output).
pub struct ResponseChunkEmitter<E: EventEmitter> {
    emitter: Arc<E>,
    run_id: String,
    seq_counter: AtomicU64,
    chunk_counter: AtomicU64,
}

impl<E: EventEmitter> ResponseChunkEmitter<E> {
    /// Create a new response chunk emitter
    pub fn new(emitter: Arc<E>, run_id: String, initial_seq: u64) -> Self {
        Self {
            emitter,
            run_id,
            seq_counter: AtomicU64::new(initial_seq),
            chunk_counter: AtomicU64::new(0),
        }
    }

    /// Emit a response chunk
    pub async fn emit_chunk(&self, content: &str, is_final: bool) {
        let _ = self
            .emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: self.run_id.clone(),
                seq: self.seq_counter.fetch_add(1, Ordering::SeqCst),
                content: content.to_string(),
                chunk_index: self.chunk_counter.fetch_add(1, Ordering::SeqCst) as u32,
                is_final,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::CollectingEventEmitter;
    use serde_json::json;

    #[tokio::test]
    async fn test_thinking_events() {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let callback = EventEmittingCallback::new(emitter.clone(), "test-run".to_string());

        callback.on_thinking_start(0).await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::Reasoning {
                content,
                is_complete,
                ..
            } => {
                assert!(content.contains("Thinking"));
                assert!(!is_complete);
            }
            _ => panic!("Expected Reasoning event"),
        }
    }

    #[tokio::test]
    async fn test_tool_lifecycle_events() {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let callback = EventEmittingCallback::new(emitter.clone(), "test-run".to_string());

        let action = Action::ToolCall {
            tool_name: "search".to_string(),
            arguments: json!({"query": "test"}),
        };

        callback.on_action_start(&action).await;

        let result = ActionResult::ToolSuccess {
            output: json!({"results": []}),
            duration_ms: 100,
        };
        callback.on_action_done(&action, &result).await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 2);

        match &events[0] {
            StreamEvent::ToolStart { tool_name, .. } => {
                assert_eq!(tool_name, "search");
            }
            _ => panic!("Expected ToolStart event"),
        }

        match &events[1] {
            StreamEvent::ToolEnd { result, .. } => {
                assert!(result.success);
            }
            _ => panic!("Expected ToolEnd event"),
        }
    }

    #[tokio::test]
    async fn test_completion_event() {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let callback = EventEmittingCallback::new(emitter.clone(), "test-run".to_string());

        callback.on_complete("Task completed successfully").await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 1);

        match &events[0] {
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                assert_eq!(content, "Task completed successfully");
                assert!(is_final);
            }
            _ => panic!("Expected ResponseChunk event"),
        }
    }

    #[tokio::test]
    async fn test_error_events() {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let callback = EventEmittingCallback::new(emitter.clone(), "test-run".to_string());

        callback.on_failed("Something went wrong").await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 1);

        match &events[0] {
            StreamEvent::RunError {
                error, error_code, ..
            } => {
                assert_eq!(error, "Something went wrong");
                assert_eq!(error_code, &Some("EXECUTION_FAILED".to_string()));
            }
            _ => panic!("Expected RunError event"),
        }
    }

    #[tokio::test]
    async fn test_sequence_numbers() {
        let emitter = Arc::new(CollectingEventEmitter::new());
        let callback = EventEmittingCallback::new(emitter.clone(), "test-run".to_string());

        callback.on_thinking_start(0).await;
        callback.on_thinking_start(1).await;
        callback.on_thinking_start(2).await;

        let events = emitter.events().await;

        let seqs: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::Reasoning { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();

        assert_eq!(seqs, vec![0, 1, 2]);
    }
}

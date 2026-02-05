//! Event Emitter for Streaming
//!
//! Provides the `EventEmitter` trait for emitting real-time streaming events
//! from the agent loop to connected WebSocket clients.

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use super::event_bus::GatewayEventBus;
use super::protocol::JsonRpcRequest;
use crate::agent_loop::thinking::{ConfidenceLevel, ReasoningStepType};

/// Streaming event types for real-time agent feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Agent run has been accepted and started
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: String,
    },

    /// Reasoning/thinking process update
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },

    /// Tool execution started
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },

    /// Tool execution progress update
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },

    /// Tool execution completed
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },

    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
    },

    /// Agent run completed successfully
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },

    /// Agent run failed with error
    RunError {
        run_id: String,
        seq: u64,
        error: String,
        error_code: Option<String>,
    },

    /// Agent is asking the user a question
    AskUser {
        run_id: String,
        seq: u64,
        question: String,
        options: Vec<String>,
    },

    /// Structured reasoning block with semantic type
    ///
    /// This is the enhanced version of the basic Reasoning event,
    /// providing semantic structure for better UI rendering.
    ReasoningBlock {
        run_id: String,
        seq: u64,
        /// Semantic step type (observation, analysis, planning, etc.)
        step_type: ReasoningStepType,
        /// Human-readable label for this block
        label: String,
        /// Content of this reasoning block
        content: String,
        /// Confidence level if determinable
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<ConfidenceLevel>,
        /// Is this the final block before action?
        is_final: bool,
    },

    /// Uncertainty signal from the AI
    ///
    /// Emitted when the AI explicitly expresses uncertainty,
    /// allowing the UI to prompt for user guidance.
    UncertaintySignal {
        run_id: String,
        seq: u64,
        /// What the AI is uncertain about
        uncertainty: String,
        /// Suggested action for handling the uncertainty
        suggested_action: UncertaintyAction,
    },
}

/// Suggested action for handling AI uncertainty
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyAction {
    /// Proceed despite uncertainty
    ProceedWithCaution,
    /// Ask user for clarification before proceeding
    AskForClarification,
    /// Use a safer/more conservative approach
    UseSaferApproach,
    /// Stop and wait for user input
    WaitForUser,
}

impl UncertaintyAction {
    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::ProceedWithCaution => "Proceeding with caution despite uncertainty",
            Self::AskForClarification => "Asking user for clarification",
            Self::UseSaferApproach => "Using a safer, more conservative approach",
            Self::WaitForUser => "Waiting for user guidance",
        }
    }
}

impl StreamEvent {
    /// Create a new ReasoningBlock event
    pub fn reasoning_block(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: None,
            is_final,
        }
    }

    /// Create a new ReasoningBlock event with confidence
    pub fn reasoning_block_with_confidence(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        confidence: ConfidenceLevel,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: Some(confidence),
            is_final,
        }
    }

    /// Create a new UncertaintySignal event
    pub fn uncertainty_signal(
        run_id: impl Into<String>,
        seq: u64,
        uncertainty: impl Into<String>,
        suggested_action: UncertaintyAction,
    ) -> Self {
        Self::UncertaintySignal {
            run_id: run_id.into(),
            seq,
            uncertainty: uncertainty.into(),
            suggested_action,
        }
    }
}

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
            metadata: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
            metadata: None,
        }
    }
}

/// Summary of a completed agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
}

/// Enhanced summary with tool details and errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
    #[serde(default)]
    pub tool_summaries: Vec<ToolSummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolErrorItem>,
}

/// Tool execution summary item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummaryItem {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,
    pub display_meta: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// Tool error item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorItem {
    pub tool_name: String,
    pub error: String,
    pub tool_id: String,
}

impl EnhancedRunSummary {
    /// Create from basic RunSummary
    pub fn from_basic(basic: &RunSummary, duration_ms: u64) -> Self {
        Self {
            total_tokens: basic.total_tokens,
            tool_calls: basic.tool_calls,
            loops: basic.loops,
            duration_ms,
            final_response: basic.final_response.clone(),
            tool_summaries: Vec::new(),
            reasoning: None,
            errors: Vec::new(),
        }
    }

    /// Add a tool summary
    pub fn add_tool(&mut self, item: ToolSummaryItem) {
        self.tool_summaries.push(item);
    }

    /// Add an error
    pub fn add_error(&mut self, error: ToolErrorItem) {
        self.errors.push(error);
    }

    /// Check if there are any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Per-RunId sequence counter manager
pub struct RunSequenceManager {
    sequences: DashMap<String, AtomicU64>,
}

impl RunSequenceManager {
    pub fn new() -> Self {
        Self {
            sequences: DashMap::new(),
        }
    }

    /// Get next sequence number for a run
    pub fn next_seq(&self, run_id: &str) -> u64 {
        self.sequences
            .entry(run_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst)
    }

    /// Cleanup sequences for completed run
    pub fn cleanup(&self, run_id: &str) {
        self.sequences.remove(run_id);
    }
}

impl Default for RunSequenceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for emitting streaming events
///
/// Implement this trait to receive real-time updates from the agent loop.
/// The default implementation broadcasts events via the Gateway event bus.
#[async_trait]
pub trait EventEmitter: Send + Sync {
    /// Emit a raw stream event
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError>;

    /// Emit a reasoning/thinking update
    async fn emit_reasoning(&self, run_id: &str, content: &str, complete: bool) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::Reasoning {
                run_id: run_id.to_string(),
                seq,
                content: content.to_string(),
                is_complete: complete,
            })
            .await;
    }

    /// Emit tool execution start
    async fn emit_tool_start(&self, run_id: &str, tool_name: &str, tool_id: &str, params: Value) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::ToolStart {
                run_id: run_id.to_string(),
                seq,
                tool_name: tool_name.to_string(),
                tool_id: tool_id.to_string(),
                params,
            })
            .await;
    }

    /// Emit tool execution progress
    async fn emit_tool_update(&self, run_id: &str, tool_id: &str, progress: &str) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::ToolUpdate {
                run_id: run_id.to_string(),
                seq,
                tool_id: tool_id.to_string(),
                progress: progress.to_string(),
            })
            .await;
    }

    /// Emit tool execution completion
    async fn emit_tool_end(&self, run_id: &str, tool_id: &str, result: ToolResult, duration_ms: u64) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::ToolEnd {
                run_id: run_id.to_string(),
                seq,
                tool_id: tool_id.to_string(),
                result,
                duration_ms,
            })
            .await;
    }

    /// Emit response text chunk
    async fn emit_response_chunk(
        &self,
        run_id: &str,
        content: &str,
        chunk_index: u32,
        is_final: bool,
    ) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq,
                content: content.to_string(),
                chunk_index,
                is_final,
            })
            .await;
    }

    /// Emit run completion
    async fn emit_run_complete(&self, run_id: &str, summary: RunSummary, duration_ms: u64) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq,
                summary,
                total_duration_ms: duration_ms,
            })
            .await;
    }

    /// Emit run error
    async fn emit_run_error(&self, run_id: &str, error: &str, error_code: Option<&str>) {
        let seq = self.next_seq();
        let _ = self
            .emit(StreamEvent::RunError {
                run_id: run_id.to_string(),
                seq,
                error: error.to_string(),
                error_code: error_code.map(|s| s.to_string()),
            })
            .await;
    }

    /// Get the next sequence number (must be monotonically increasing)
    fn next_seq(&self) -> u64;
}

/// Error type for event emission
#[derive(Debug, thiserror::Error)]
pub enum EventEmitError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Event bus error: {0}")]
    EventBus(String),
}

/// Gateway-based event emitter
///
/// Broadcasts events to all connected WebSocket clients via the event bus.
/// Supports throttled response chunk emission (150ms) for smoother streaming.
pub struct GatewayEventEmitter {
    event_bus: Arc<GatewayEventBus>,
    seq_counter: AtomicU64,
    // Throttling state for response chunks
    delta_buffer: Mutex<String>,
    last_delta_at: Mutex<Instant>,
}

impl GatewayEventEmitter {
    /// Delta event throttle interval (150ms like OpenClaw)
    const DELTA_THROTTLE_MS: u64 = 150;

    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            delta_buffer: Mutex::new(String::new()),
            last_delta_at: Mutex::new(Instant::now()),
        }
    }

    /// Emit response chunk with 150ms throttling
    ///
    /// Buffers chunks within the throttle window, sends accumulated content on boundary.
    /// Final chunks are always sent immediately with any buffered content.
    pub async fn emit_response_chunk_throttled(
        &self,
        run_id: &str,
        content: &str,
        chunk_index: u32,
        is_final: bool,
    ) {
        if is_final {
            // Always send final chunk immediately with any buffered content
            let mut buffer = self.delta_buffer.lock().await;
            let full_content = if buffer.is_empty() {
                content.to_string()
            } else {
                let buffered = std::mem::take(&mut *buffer);
                format!("{}{}", buffered, content)
            };
            drop(buffer);

            self.emit_response_chunk(run_id, &full_content, chunk_index, true)
                .await;
            return;
        }

        let now = Instant::now();
        let mut last_at = self.last_delta_at.lock().await;
        let elapsed = now.duration_since(*last_at).as_millis() as u64;

        if elapsed < Self::DELTA_THROTTLE_MS {
            // Buffer the content, don't send yet
            self.delta_buffer.lock().await.push_str(content);
            return;
        }

        // Send buffered + new content
        let mut buffer = self.delta_buffer.lock().await;
        let full_content = if buffer.is_empty() {
            content.to_string()
        } else {
            let buffered = std::mem::take(&mut *buffer);
            format!("{}{}", buffered, content)
        };
        drop(buffer);

        *last_at = now;
        drop(last_at);

        self.emit_response_chunk(run_id, &full_content, chunk_index, false)
            .await;
    }
}

#[async_trait]
impl EventEmitter for GatewayEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Wrap as JSON-RPC notification
        let event_value = serde_json::to_value(&event)?;
        let notification = JsonRpcRequest::notification(event_method(&event), Some(event_value));
        let json = serde_json::to_string(&notification)?;
        self.event_bus.publish(json);
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// No-op event emitter for testing or when streaming is disabled
pub struct NoOpEventEmitter {
    seq_counter: AtomicU64,
}

impl NoOpEventEmitter {
    pub fn new() -> Self {
        Self {
            seq_counter: AtomicU64::new(0),
        }
    }
}

impl Default for NoOpEventEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventEmitter for NoOpEventEmitter {
    async fn emit(&self, _event: StreamEvent) -> Result<(), EventEmitError> {
        // Do nothing
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Collecting event emitter for testing
///
/// Stores all emitted events for later inspection.
pub struct CollectingEventEmitter {
    events: tokio::sync::Mutex<Vec<StreamEvent>>,
    seq_counter: AtomicU64,
}

impl CollectingEventEmitter {
    pub fn new() -> Self {
        Self {
            events: tokio::sync::Mutex::new(Vec::new()),
            seq_counter: AtomicU64::new(0),
        }
    }

    pub async fn events(&self) -> Vec<StreamEvent> {
        self.events.lock().await.clone()
    }

    pub async fn clear(&self) {
        self.events.lock().await.clear();
    }
}

impl Default for CollectingEventEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventEmitter for CollectingEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.events.lock().await.push(event);
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Wrapper for dynamic EventEmitter trait objects
///
/// This wrapper allows passing `Arc<dyn EventEmitter + Send + Sync>` to generic
/// functions that require `E: EventEmitter + Send + Sync + 'static`.
/// The wrapper is Sized and delegates all calls to the inner trait object.
pub struct DynEventEmitter {
    inner: Arc<dyn EventEmitter + Send + Sync>,
}

impl DynEventEmitter {
    /// Create a new wrapper around a dynamic EventEmitter
    pub fn new(emitter: Arc<dyn EventEmitter + Send + Sync>) -> Self {
        Self { inner: emitter }
    }
}

#[async_trait]
impl EventEmitter for DynEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.inner.emit(event).await
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}

/// Get the JSON-RPC method name for a stream event
fn event_method(event: &StreamEvent) -> &'static str {
    match event {
        StreamEvent::RunAccepted { .. } => "stream.run_accepted",
        StreamEvent::Reasoning { .. } => "stream.reasoning",
        StreamEvent::ToolStart { .. } => "stream.tool_start",
        StreamEvent::ToolUpdate { .. } => "stream.tool_update",
        StreamEvent::ToolEnd { .. } => "stream.tool_end",
        StreamEvent::ResponseChunk { .. } => "stream.response_chunk",
        StreamEvent::RunComplete { .. } => "stream.run_complete",
        StreamEvent::RunError { .. } => "stream.run_error",
        StreamEvent::AskUser { .. } => "stream.ask_user",
        StreamEvent::ReasoningBlock { .. } => "stream.reasoning_block",
        StreamEvent::UncertaintySignal { .. } => "stream.uncertainty_signal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collecting_emitter() {
        let emitter = CollectingEventEmitter::new();

        emitter.emit_reasoning("run-1", "Thinking...", false).await;
        emitter.emit_reasoning("run-1", "Done thinking", true).await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 2);

        match &events[0] {
            StreamEvent::Reasoning { content, is_complete, .. } => {
                assert_eq!(content, "Thinking...");
                assert!(!is_complete);
            }
            _ => panic!("Expected Reasoning event"),
        }
    }

    #[tokio::test]
    async fn test_sequence_numbers() {
        let emitter = CollectingEventEmitter::new();

        emitter.emit_reasoning("run-1", "First", false).await;
        emitter.emit_reasoning("run-1", "Second", false).await;
        emitter.emit_reasoning("run-1", "Third", true).await;

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

    #[tokio::test]
    async fn test_tool_lifecycle() {
        let emitter = CollectingEventEmitter::new();

        emitter
            .emit_tool_start("run-1", "read_file", "tool-1", serde_json::json!({"path": "/tmp/test"}))
            .await;
        emitter
            .emit_tool_update("run-1", "tool-1", "Reading file...")
            .await;
        emitter
            .emit_tool_end("run-1", "tool-1", ToolResult::success("file contents"), 100)
            .await;

        let events = emitter.events().await;
        assert_eq!(events.len(), 3);

        assert!(matches!(&events[0], StreamEvent::ToolStart { .. }));
        assert!(matches!(&events[1], StreamEvent::ToolUpdate { .. }));
        assert!(matches!(&events[2], StreamEvent::ToolEnd { .. }));
    }

    #[test]
    fn test_event_method_names() {
        let event = StreamEvent::Reasoning {
            run_id: "".to_string(),
            seq: 0,
            content: "".to_string(),
            is_complete: false,
        };
        assert_eq!(event_method(&event), "stream.reasoning");

        let event = StreamEvent::ToolStart {
            run_id: "".to_string(),
            seq: 0,
            tool_name: "".to_string(),
            tool_id: "".to_string(),
            params: serde_json::json!({}),
        };
        assert_eq!(event_method(&event), "stream.tool_start");
    }

    #[tokio::test]
    async fn test_throttled_response_chunk_buffering() {
        use super::super::event_bus::GatewayEventBus;

        let event_bus = Arc::new(GatewayEventBus::new());
        let emitter = GatewayEventEmitter::new(event_bus);

        // First chunk should be sent (enough time elapsed from initialization)
        emitter
            .emit_response_chunk_throttled("run-1", "Hello ", 0, false)
            .await;

        // These should be buffered (within 150ms window)
        emitter
            .emit_response_chunk_throttled("run-1", "World", 1, false)
            .await;
        emitter
            .emit_response_chunk_throttled("run-1", "!", 2, false)
            .await;

        // Check that content is buffered
        let buffer = emitter.delta_buffer.lock().await;
        assert!(
            buffer.contains("World") || buffer.contains("!"),
            "Buffer should contain throttled content"
        );
        drop(buffer);

        // Final chunk should flush everything
        emitter
            .emit_response_chunk_throttled("run-1", " Done", 3, true)
            .await;

        // Buffer should be empty after final
        let buffer = emitter.delta_buffer.lock().await;
        assert!(buffer.is_empty(), "Buffer should be empty after final chunk");
    }

    #[tokio::test]
    async fn test_throttled_response_chunk_final_always_sends() {
        use super::super::event_bus::GatewayEventBus;

        let event_bus = Arc::new(GatewayEventBus::new());
        let emitter = GatewayEventEmitter::new(event_bus);

        // Add some content to buffer
        {
            let mut buffer = emitter.delta_buffer.lock().await;
            buffer.push_str("buffered content ");
        }

        // Final should include buffered content
        emitter
            .emit_response_chunk_throttled("run-1", "final", 0, true)
            .await;

        // Buffer should be cleared
        let buffer = emitter.delta_buffer.lock().await;
        assert!(buffer.is_empty(), "Buffer should be empty after final");
    }

    #[test]
    fn test_throttle_constant() {
        assert_eq!(
            GatewayEventEmitter::DELTA_THROTTLE_MS,
            150,
            "Throttle interval should be 150ms"
        );
    }

    #[test]
    fn test_reasoning_block_serialization() {
        let event = StreamEvent::reasoning_block(
            "run-123",
            1,
            ReasoningStepType::Analysis,
            "Analyzing options",
            "Comparing Redis vs in-memory cache",
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reasoning_block"));
        assert!(json.contains("analysis"));
        assert!(json.contains("Analyzing options"));
    }

    #[test]
    fn test_reasoning_block_with_confidence() {
        let event = StreamEvent::reasoning_block_with_confidence(
            "run-123",
            2,
            ReasoningStepType::Decision,
            "Final decision",
            "Will use Redis",
            ConfidenceLevel::High,
            true,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("decision"));
        assert!(json.contains("high"));
        assert!(json.contains("is_final"));
    }

    #[test]
    fn test_uncertainty_signal() {
        let event = StreamEvent::uncertainty_signal(
            "run-123",
            3,
            "Not sure about the caching strategy",
            UncertaintyAction::AskForClarification,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("uncertainty_signal"));
        assert!(json.contains("ask_for_clarification"));
    }

    #[test]
    fn test_uncertainty_action_description() {
        assert!(UncertaintyAction::ProceedWithCaution.description().contains("caution"));
        assert!(UncertaintyAction::AskForClarification.description().contains("clarification"));
    }

    #[test]
    fn test_deserialize_reasoning_block() {
        let json = r#"{"type":"reasoning_block","run_id":"r1","seq":1,"step_type":"observation","label":"Look","content":"Seeing the code","confidence":null,"is_final":false}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();

        if let StreamEvent::ReasoningBlock { step_type, label, .. } = event {
            assert_eq!(step_type, ReasoningStepType::Observation);
            assert_eq!(label, "Look");
        } else {
            panic!("Wrong event type");
        }
    }
}

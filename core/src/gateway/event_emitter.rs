//! Event Emitter for Streaming
//!
//! Provides the `EventEmitter` trait for emitting real-time streaming events
//! from the agent loop to connected WebSocket clients.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::event_bus::GatewayEventBus;
use super::protocol::JsonRpcRequest;

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
pub struct GatewayEventEmitter {
    event_bus: Arc<GatewayEventBus>,
    seq_counter: AtomicU64,
}

impl GatewayEventEmitter {
    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
        }
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
}

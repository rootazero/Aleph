//! Reply Emitter - Routes Agent output back to channels
//!
//! The ReplyEmitter implements EventEmitter to capture streaming events from the
//! agent loop and route responses back to the originating channel/conversation.
//!
//! # Features
//!
//! - Buffers response chunks to avoid sending too many small messages
//! - Flushes buffer when threshold is reached or on completion
//! - Handles errors gracefully, sending error messages to users
//! - Supports optional streaming mode for channels that support it
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::gateway::{ReplyEmitter, ChannelRegistry, ReplyRoute};
//!
//! let emitter = ReplyEmitter::new(
//!     channel_registry.clone(),
//!     reply_route,
//!     "run-123".to_string(),
//! );
//!
//! // Use emitter as EventEmitter for agent execution
//! execution_engine.run_with_emitter(request, Arc::new(emitter)).await;
//! ```

use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use super::channel::OutboundMessage;
use super::channel_registry::ChannelRegistry;
use super::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use super::inbound_context::ReplyRoute;

/// Configuration for ReplyEmitter behavior
#[derive(Debug, Clone)]
pub struct ReplyEmitterConfig {
    /// Minimum buffer size before auto-flush (in characters)
    /// Default: 500 characters
    pub buffer_threshold: usize,

    /// Whether to stream responses to the channel
    /// Default: false (iMessage and most channels don't handle streaming well)
    pub stream_enabled: bool,
}

impl Default for ReplyEmitterConfig {
    fn default() -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: false,
        }
    }
}

/// Routes Agent output back to the originating channel/conversation
///
/// ReplyEmitter captures streaming events from the agent loop and accumulates
/// response text, then sends it back to the user via the appropriate channel.
pub struct ReplyEmitter {
    /// Channel registry for sending messages
    channel_registry: Arc<ChannelRegistry>,

    /// Route for sending replies back
    route: ReplyRoute,

    /// Configuration
    config: ReplyEmitterConfig,

    /// Buffer for accumulating response text
    buffer: Mutex<String>,

    /// Sequence counter for events
    seq_counter: AtomicU64,

    /// Run ID for this execution
    run_id: String,
}

impl ReplyEmitter {
    /// Create a new ReplyEmitter with default configuration
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
    ) -> Self {
        Self {
            channel_registry,
            route,
            config: ReplyEmitterConfig::default(),
            buffer: Mutex::new(String::new()),
            seq_counter: AtomicU64::new(0),
            run_id,
        }
    }

    /// Create a new ReplyEmitter with custom configuration
    pub fn with_config(
        channel_registry: Arc<ChannelRegistry>,
        route: ReplyRoute,
        run_id: String,
        config: ReplyEmitterConfig,
    ) -> Self {
        Self {
            channel_registry,
            route,
            config,
            buffer: Mutex::new(String::new()),
            seq_counter: AtomicU64::new(0),
            run_id,
        }
    }

    /// Get the run ID
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Get the reply route
    pub fn route(&self) -> &ReplyRoute {
        &self.route
    }

    /// Buffer text content
    ///
    /// If the buffer exceeds the threshold, it will be automatically flushed.
    async fn buffer_text(&self, text: &str) {
        let mut buffer = self.buffer.lock().await;
        buffer.push_str(text);

        // Auto-flush if threshold exceeded and streaming is enabled
        if self.config.stream_enabled && buffer.len() >= self.config.buffer_threshold {
            let content = std::mem::take(&mut *buffer);
            drop(buffer); // Release lock before sending
            self.send_to_channel(&content).await;
        }
    }

    /// Flush the buffer, sending accumulated content to the channel
    async fn flush(&self) {
        let mut buffer = self.buffer.lock().await;
        if buffer.is_empty() {
            return;
        }

        let content = std::mem::take(&mut *buffer);
        drop(buffer); // Release lock before sending

        self.send_to_channel(&content).await;
    }

    /// Send content to the channel
    async fn send_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: content.to_string(),
            attachments: vec![],
            reply_to: self.route.reply_to.clone(),
            inline_keyboard: None,
            metadata: Default::default(),
        };

        match self
            .channel_registry
            .send(&self.route.channel_id, message)
            .await
        {
            Ok(result) => {
                debug!(
                    "Sent reply to channel {} (message_id: {})",
                    self.route.channel_id,
                    result.message_id.as_str()
                );
            }
            Err(e) => {
                error!(
                    "Failed to send reply to channel {}: {}",
                    self.route.channel_id, e
                );
            }
        }
    }

    /// Send an error message to the user
    async fn send_error(&self, error: &str) {
        let error_message = format!("Error: {}", error);
        self.send_to_channel(&error_message).await;
    }
}

#[async_trait]
impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match event {
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                // Buffer the response text
                self.buffer_text(&content).await;

                // Flush on final chunk
                if is_final {
                    self.flush().await;
                }
            }

            StreamEvent::RunComplete { summary, .. } => {
                // Flush any remaining buffer
                self.flush().await;

                // If there's a final response in the summary that wasn't streamed,
                // send it now (this handles non-streaming mode)
                if let Some(final_response) = summary.final_response {
                    // Check if buffer was empty (meaning response wasn't streamed)
                    let buffer = self.buffer.lock().await;
                    if buffer.is_empty() {
                        drop(buffer);
                        // Only send if we haven't already sent something
                        // The final_response might duplicate what was in ResponseChunks
                        debug!(
                            "Run {} complete with final_response length: {}",
                            self.run_id,
                            final_response.len()
                        );
                    }
                }
            }

            StreamEvent::RunError { error, .. } => {
                // Flush any partial response
                self.flush().await;

                // Send error message to user
                warn!("Run {} failed: {}", self.run_id, error);
                self.send_error(&error).await;
            }

            StreamEvent::AskUser { question, .. } => {
                // Flush buffer first
                self.flush().await;

                // Send the question to the user
                self.send_to_channel(&question).await;
            }

            // Other events are not routed to the channel
            StreamEvent::RunAccepted { .. }
            | StreamEvent::Reasoning { .. }
            | StreamEvent::ToolStart { .. }
            | StreamEvent::ToolUpdate { .. }
            | StreamEvent::ToolEnd { .. }
            | StreamEvent::ReasoningBlock { .. }
            | StreamEvent::UncertaintySignal { .. } => {
                // These events are for WebSocket clients, not channel users
                debug!("Ignoring event for channel routing: {:?}", event);
            }
        }

        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId};

    #[test]
    fn test_config_defaults() {
        let config = ReplyEmitterConfig::default();
        assert_eq!(config.buffer_threshold, 500);
        assert!(!config.stream_enabled);
    }

    #[test]
    fn test_reply_route() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(registry, route.clone(), "run-123".to_string());

        assert_eq!(emitter.run_id(), "run-123");
        assert_eq!(emitter.route().channel_id.as_str(), "imessage");
        assert_eq!(emitter.route().conversation_id.as_str(), "+15551234567");
    }

    #[test]
    fn test_custom_config() {
        let route = ReplyRoute::new(
            ChannelId::new("telegram"),
            ConversationId::new("12345"),
        );

        let config = ReplyEmitterConfig {
            buffer_threshold: 1000,
            stream_enabled: true,
        };

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::with_config(
            registry,
            route,
            "run-456".to_string(),
            config,
        );

        assert_eq!(emitter.config.buffer_threshold, 1000);
        assert!(emitter.config.stream_enabled);
    }

    #[tokio::test]
    async fn test_sequence_counter() {
        let route = ReplyRoute::new(
            ChannelId::new("test"),
            ConversationId::new("conv-1"),
        );

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(registry, route, "run-789".to_string());

        // Sequence should start at 0 and increment
        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
        assert_eq!(emitter.next_seq(), 2);
    }

    #[tokio::test]
    async fn test_buffer_accumulation() {
        let route = ReplyRoute::new(
            ChannelId::new("test"),
            ConversationId::new("conv-1"),
        );

        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(registry, route, "run-test".to_string());

        // Buffer some text
        emitter.buffer_text("Hello ").await;
        emitter.buffer_text("World!").await;

        // Check buffer contents
        let buffer = emitter.buffer.lock().await;
        assert_eq!(*buffer, "Hello World!");
    }
}

//! Reply Emitter - Routes Agent output back to channels
//!
//! The ReplyEmitter implements EventEmitter to capture streaming events from the
//! agent loop and route responses back to the originating channel/conversation.
//!
//! # Output Modes
//!
//! - **Typewriter** (`stream_enabled = true`): Sends an initial message, then
//!   progressively edits it with more content, creating a streaming visual effect.
//!   Falls back to instant behavior if the channel doesn't support editing.
//! - **Instant** (`stream_enabled = false`): Buffers all content, sends once on
//!   completion.
//!
//! Mode is controlled by `BehaviorConfig.output_mode` in config.toml.

use std::time::Duration;

use async_trait::async_trait;
use crate::sync_primitives::{AtomicBool, AtomicU64, Ordering};
use crate::sync_primitives::Arc;
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

    /// Whether to stream responses to the channel (typewriter mode)
    /// Default: false
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

impl ReplyEmitterConfig {
    /// Create config from output_mode string ("typewriter" or "instant")
    pub fn from_output_mode(mode: &str) -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: mode == "typewriter",
        }
    }
}

/// Routes Agent output back to the originating channel/conversation.
///
/// In typewriter mode, all response chunks are buffered silently. When
/// `RunComplete` fires, the emitter sends an initial message and then
/// progressively edits it with more content (300ms intervals), creating
/// a streaming visual similar to ChatGPT's Telegram bot.
///
/// In instant mode, the full response is sent as one message on completion.
pub struct ReplyEmitter {
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    config: ReplyEmitterConfig,

    /// Accumulated response text (both modes)
    buffer: Mutex<String>,

    seq_counter: AtomicU64,
    has_sent: AtomicBool,
    run_id: String,
}

/// Interval between progressive edits in typewriter mode.
const TYPEWRITER_EDIT_INTERVAL: Duration = Duration::from_millis(300);

/// Number of characters to reveal per edit step.
const TYPEWRITER_CHARS_PER_STEP: usize = 80;

impl ReplyEmitter {
    /// Create a new ReplyEmitter with default configuration (instant mode)
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
            has_sent: AtomicBool::new(false),
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
            has_sent: AtomicBool::new(false),
            run_id,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn route(&self) -> &ReplyRoute {
        &self.route
    }

    fn format_content(&self, content: &str, _is_first: bool) -> String {
        content.to_string()
    }

    // ── Typewriter mode ─────────────────────────────────────────────────

    /// Send the buffered content with progressive edit-in-place streaming.
    ///
    /// 1. Send initial message with a small portion of the text
    /// 2. Edit the message with progressively more content (every 300ms)
    /// 3. Final edit with the complete text
    ///
    /// Falls back to instant send if the channel doesn't support editing.
    async fn send_typewriter(&self, full_text: &str) {
        if full_text.is_empty() {
            return;
        }

        let formatted = self.format_content(full_text, true);
        self.has_sent.store(true, Ordering::SeqCst);

        // For short responses, just send directly (no need for progressive editing)
        if formatted.chars().count() <= TYPEWRITER_CHARS_PER_STEP * 2 {
            self.send_to_channel(&formatted).await;
            return;
        }

        // Step 1: Send initial message with first portion
        let char_indices: Vec<usize> = formatted.char_indices().map(|(i, _)| i).collect();
        let total_chars = char_indices.len();
        let first_end = TYPEWRITER_CHARS_PER_STEP.min(total_chars);
        let first_byte_end = if first_end < total_chars {
            char_indices[first_end]
        } else {
            formatted.len()
        };
        let initial_text = &formatted[..first_byte_end];

        // Append "▍" cursor indicator
        let initial_with_cursor = format!("{}▍", initial_text);

        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: initial_with_cursor,
            attachments: vec![],
            reply_to: self.route.reply_to.clone(),
            inline_keyboard: None,
            metadata: Default::default(),
        };

        let msg_id = match self.channel_registry.send(&self.route.channel_id, message).await {
            Ok(result) => {
                debug!(
                    "Typewriter: sent initial message {} ({}B)",
                    result.message_id.as_str(),
                    initial_text.len()
                );
                result.message_id
            }
            Err(e) => {
                error!("Typewriter: failed to send initial message: {}", e);
                // Fall back: send everything at once
                self.send_to_channel(&formatted).await;
                return;
            }
        };

        // Step 2: Progressive edits
        let mut revealed_chars = first_end;

        while revealed_chars < total_chars {
            tokio::time::sleep(TYPEWRITER_EDIT_INTERVAL).await;

            revealed_chars = (revealed_chars + TYPEWRITER_CHARS_PER_STEP).min(total_chars);
            let byte_end = if revealed_chars < total_chars {
                char_indices[revealed_chars]
            } else {
                formatted.len()
            };

            let partial = &formatted[..byte_end];
            let edit_text = if revealed_chars < total_chars {
                format!("{}▍", partial) // Still typing
            } else {
                partial.to_string() // Done — remove cursor
            };

            match self.channel_registry.edit(
                &self.route.channel_id,
                &self.route.conversation_id,
                &msg_id,
                &edit_text,
            ).await {
                Ok(()) => {
                    debug!(
                        "Typewriter: edited message ({}/{} chars)",
                        revealed_chars, total_chars
                    );
                }
                Err(e) => {
                    // Edit failed — likely unsupported channel, stop trying
                    debug!("Typewriter: edit failed ({}), stopping progressive edits", e);
                    break;
                }
            }
        }

        // Step 3: Final edit to ensure complete text is shown (without cursor)
        let _ = self.channel_registry.edit(
            &self.route.channel_id,
            &self.route.conversation_id,
            &msg_id,
            &formatted,
        ).await;
    }

    // ── Shared helpers ──────────────────────────────────────────────────

    const MAX_MESSAGE_LENGTH: usize = 4000;

    /// Send content to the channel, splitting into chunks if too long (instant mode)
    async fn send_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let is_first_send = !self.has_sent.load(Ordering::SeqCst);
        self.has_sent.store(true, Ordering::SeqCst);

        let content = self.format_content(content, is_first_send);

        let chunks = Self::split_message(&content, Self::MAX_MESSAGE_LENGTH);
        let total_chunks = chunks.len();

        for (i, chunk) in chunks.into_iter().enumerate() {
            let message = OutboundMessage {
                conversation_id: self.route.conversation_id.clone(),
                text: chunk,
                attachments: vec![],
                reply_to: if i == 0 {
                    self.route.reply_to.clone()
                } else {
                    None
                },
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
                        "Sent reply to channel {} (message_id: {}, chunk {}/{})",
                        self.route.channel_id,
                        result.message_id.as_str(),
                        i + 1,
                        total_chunks
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to send reply to channel {} (chunk {}/{}): {}",
                        self.route.channel_id,
                        i + 1,
                        total_chunks,
                        e
                    );
                    break;
                }
            }
        }
    }

    fn split_message(content: &str, max_len: usize) -> Vec<String> {
        if content.len() <= max_len {
            return vec![content.to_string()];
        }

        let mut chunks = Vec::new();
        let mut remaining = content;

        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                chunks.push(remaining.to_string());
                break;
            }

            let split_at = Self::find_split_point(remaining, max_len);
            let (chunk, rest) = remaining.split_at(split_at);
            chunks.push(chunk.trim_end().to_string());
            remaining = rest.trim_start_matches('\n');
        }

        chunks
    }

    fn find_split_point(text: &str, max_len: usize) -> usize {
        let mut safe_max = max_len;
        while safe_max > 0 && !text.is_char_boundary(safe_max) {
            safe_max -= 1;
        }

        let search_range = &text[..safe_max];

        if let Some(pos) = search_range.rfind("\n\n") {
            if pos > safe_max / 4 {
                return pos + 1;
            }
        }

        if let Some(pos) = search_range.rfind('\n') {
            if pos > safe_max / 4 {
                return pos + 1;
            }
        }

        safe_max
    }

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
                // Both modes: accumulate text into buffer
                if !content.is_empty() {
                    self.buffer.lock().await.push_str(&content);
                }

                // Instant mode: flush on final chunk
                if !self.config.stream_enabled && is_final {
                    let mut buffer = self.buffer.lock().await;
                    if !buffer.is_empty() {
                        let text = std::mem::take(&mut *buffer);
                        drop(buffer);
                        self.send_to_channel(&text).await;
                    }
                }
                // Typewriter mode: do nothing here, wait for RunComplete
            }

            StreamEvent::RunComplete { summary, .. } => {
                // Flush accumulated buffer
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };

                if !text.is_empty() && !self.has_sent.load(Ordering::SeqCst) {
                    if self.config.stream_enabled {
                        self.send_typewriter(&text).await;
                    } else {
                        self.send_to_channel(&text).await;
                    }
                }

                // Fallback: if nothing was sent, use final_response from summary
                if !self.has_sent.load(Ordering::SeqCst) {
                    if let Some(ref final_response) = summary.final_response {
                        if !final_response.is_empty() {
                            debug!(
                                "Run {} complete, sending final_response as fallback (length: {})",
                                self.run_id,
                                final_response.len()
                            );
                            if self.config.stream_enabled {
                                self.send_typewriter(final_response).await;
                            } else {
                                self.send_to_channel(final_response).await;
                            }
                        }
                    }
                }
            }

            StreamEvent::RunError { error, .. } => {
                // Flush any partial response
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                if !text.is_empty() {
                    self.send_to_channel(&text).await;
                }

                warn!("Run {} failed: {}", self.run_id, error);
                self.send_error(&error).await;
            }

            StreamEvent::AskUser { question, .. } => {
                // Flush buffer first
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                if !text.is_empty() {
                    self.send_to_channel(&text).await;
                }

                self.send_to_channel(&question).await;
            }

            // Other events are not routed to channels
            StreamEvent::RunAccepted { .. }
            | StreamEvent::Reasoning { .. }
            | StreamEvent::ToolStart { .. }
            | StreamEvent::ToolUpdate { .. }
            | StreamEvent::ToolEnd { .. }
            | StreamEvent::ReasoningBlock { .. }
            | StreamEvent::UncertaintySignal { .. }
            | StreamEvent::SessionUpdated { .. } => {
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
    fn test_config_from_output_mode() {
        let tw = ReplyEmitterConfig::from_output_mode("typewriter");
        assert!(tw.stream_enabled);

        let instant = ReplyEmitterConfig::from_output_mode("instant");
        assert!(!instant.stream_enabled);
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

        emitter.buffer.lock().await.push_str("Hello ");
        emitter.buffer.lock().await.push_str("World!");

        let buffer = emitter.buffer.lock().await;
        assert_eq!(*buffer, "Hello World!");
    }

    #[test]
    fn test_split_message_short() {
        let chunks = ReplyEmitter::split_message("Hello World", 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello World");
    }

    #[test]
    fn test_split_message_at_paragraph() {
        let content = format!("{}\n\n{}", "A".repeat(50), "B".repeat(50));
        let chunks = ReplyEmitter::split_message(&content, 60);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with('A'));
        assert!(chunks[1].starts_with('B'));
    }

    #[test]
    fn test_split_message_at_newline() {
        let content = format!("{}\n{}", "A".repeat(50), "B".repeat(50));
        let chunks = ReplyEmitter::split_message(&content, 60);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_split_message_no_boundary() {
        let content = "A".repeat(200);
        let chunks = ReplyEmitter::split_message(&content, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
    }

    #[test]
    fn test_split_message_utf8_safe() {
        let content = "中".repeat(100);
        let chunks = ReplyEmitter::split_message(&content, 150);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 150);
        }
    }

}

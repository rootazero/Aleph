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
use tokio_util::sync::CancellationToken;
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

    /// Whether voice output is enabled for this emitter
    /// Default: false
    pub voice_enabled: bool,

    /// Whether the inbound message requested a voice reply
    /// Default: false
    pub voice_reply_hint: bool,
}

impl Default for ReplyEmitterConfig {
    fn default() -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: false,
            voice_enabled: false,
            voice_reply_hint: false,
        }
    }
}

impl ReplyEmitterConfig {
    /// Create config from output_mode string ("typewriter" or "instant")
    pub fn from_output_mode(mode: &str) -> Self {
        Self {
            buffer_threshold: 500,
            stream_enabled: mode == "typewriter",
            voice_enabled: false,
            voice_reply_hint: false,
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

    /// Cancellation token to stop the persistent typing indicator task
    typing_cancel: CancellationToken,

    /// Generation provider registry for TTS (voice mode)
    generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
    /// Generation config for TTS (voice mode)
    generation_config: Option<Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>>,
    /// Per-channel voice state
    voice_state: Mutex<super::voice::VoiceState>,
}

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
            typing_cancel: CancellationToken::new(),
            generation_registry: None,
            generation_config: None,
            voice_state: Mutex::new(Default::default()),
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
            typing_cancel: CancellationToken::new(),
            generation_registry: None,
            generation_config: None,
            voice_state: Mutex::new(Default::default()),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn route(&self) -> &ReplyRoute {
        &self.route
    }

    /// Attach voice mode dependencies, enabling TTS output.
    pub fn with_voice(
        mut self,
        voice_state: super::voice::VoiceState,
        generation_registry: Arc<crate::generation::GenerationProviderRegistry>,
        generation_config: Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>,
    ) -> Self {
        self.voice_state = Mutex::new(voice_state);
        self.generation_registry = Some(generation_registry);
        self.generation_config = Some(generation_config);
        self
    }

    /// Returns true when voice output should be attempted.
    /// Dynamically check if voice output should be used.
    ///
    /// Re-reads VoiceState from ChannelRegistry on every call so that
    /// mid-request voice_mode_set tool calls take effect immediately
    /// (e.g. the confirmation message itself is voiced).
    async fn should_voice(&self) -> bool {
        // Static hint: user sent an audio message
        if self.config.voice_reply_hint {
            debug!("should_voice=true (voice_reply_hint)");
            return true;
        }
        // Dynamic check: read current voice state from registry
        // Check both the specific channel and "default" (fallback when
        // voice_mode_set tool is called without explicit channel_id)
        let channel_id = self.route.channel_id.as_str();
        let live_state = self.channel_registry.get_voice_state(channel_id).await;
        debug!("should_voice: channel={}, enabled={}", channel_id, live_state.is_active());
        if live_state.is_active() {
            return true;
        }
        // Fallback: check "default" key (used when tool doesn't know channel)
        if channel_id != "default" {
            let default_state = self.channel_registry.get_voice_state("default").await;
            debug!("should_voice: fallback 'default' enabled={}, has_gen_registry={}", default_state.is_active(), self.generation_registry.is_some());
            if default_state.is_active() {
                debug!("should_voice => TRUE (default fallback)");
                return true;
            }
        }
        debug!("should_voice => FALSE");
        false
    }

    /// Try to generate TTS audio and send as a voice message.
    ///
    /// On success the message includes both text and an audio attachment.
    /// On failure it records the failure on `voice_state` and falls back to
    /// plain text via `send_to_channel`.
    async fn send_as_voice(&self, text: &str) {
        debug!("send_as_voice called, text_len={}, has_gen_registry={}", text.len(), self.generation_registry.is_some());
        if let Some(ref registry) = self.generation_registry {
            // Read live voice state from registry (not the stale local copy)
            // Check channel-specific first, then "default" fallback
            let channel_id = self.route.channel_id.as_str();
            let mut voice_state = self.channel_registry.get_voice_state(channel_id).await;
            if !voice_state.is_active() && channel_id != "default" {
                voice_state = self.channel_registry.get_voice_state("default").await;
            }
            let gen_config = match self.generation_config {
                Some(ref cfg) => cfg.read().await.clone(),
                None => {
                    self.send_to_channel(text).await;
                    return;
                }
            };

            if let Some(attachment) = super::voice::outbound::generate_tts(
                text,
                &voice_state,
                registry,
                &gen_config,
            )
            .await
            {
                // Success — reset failure counter
                self.voice_state.lock().await.record_success();

                let message = OutboundMessage {
                    conversation_id: self.route.conversation_id.clone(),
                    text: text.to_string(),
                    attachments: vec![attachment],
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
                            "Sent voice reply to channel {} (message_id: {})",
                            self.route.channel_id,
                            result.message_id.as_str(),
                        );
                        self.has_sent.store(true, Ordering::SeqCst);
                    }
                    Err(e) => {
                        error!("Failed to send voice reply: {}", e);
                        // Fall back to text
                        self.send_to_channel(text).await;
                    }
                }
            } else {
                // TTS generation failed — record and maybe auto-disable
                let auto_disabled = self.voice_state.lock().await.record_failure();
                if auto_disabled {
                    warn!(
                        "Voice auto-disabled for channel {} after 3 consecutive TTS failures",
                        self.route.channel_id
                    );
                }
                // Fallback to plain text
                self.send_to_channel(text).await;
            }
        } else {
            // No generation registry — fall back to text
            self.send_to_channel(text).await;
        }
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
            // Adaptive interval: accelerate in the second half of the message
            let progress = revealed_chars as f64 / total_chars as f64;
            let interval_ms = if progress > 0.5 {
                let extra = ((progress - 0.5) * 2.0 * 200.0) as u64;
                300 + extra.min(200)
            } else {
                300
            };
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;

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
        let mut buf = content.to_string();
        let mut open_fence: Option<String> = None; // e.g. "```rust"

        while !buf.is_empty() {
            if buf.len() <= max_len {
                chunks.push(buf);
                break;
            }

            let split_at = Self::find_split_point(&buf, max_len);
            let chunk_raw = buf[..split_at].trim_end().to_string();
            let rest = buf[split_at..].trim_start_matches('\n').to_string();

            // Track code fence state through this chunk
            for line in chunk_raw.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("```") {
                    if open_fence.is_some() {
                        open_fence = None;
                    } else {
                        open_fence = Some(trimmed.to_string());
                    }
                }
            }

            // If we're splitting inside an open code block, close + reopen
            if let Some(ref fence) = open_fence {
                let mut chunk_closed = chunk_raw;
                chunk_closed.push_str("\n```");
                chunks.push(chunk_closed);
                buf = format!("{}\n{}", fence, rest);
            } else {
                chunks.push(chunk_raw);
                buf = rest;
            }
        }

        chunks
    }

    fn find_split_point(text: &str, max_len: usize) -> usize {
        let mut safe_max = max_len;
        while safe_max > 0 && !text.is_char_boundary(safe_max) {
            safe_max -= 1;
        }

        let search_range = &text[..safe_max];

        // Check if we're inside a code block (odd number of ``` before split point)
        let fence_count = search_range.matches("```").count();
        if fence_count % 2 == 1 {
            // Inside a code block — try to split before the opening fence
            if let Some(last_fence) = search_range.rfind("```") {
                if last_fence > 0 {
                    if let Some(nl) = text[..last_fence].rfind('\n') {
                        return nl + 1;
                    }
                    return last_fence;
                }
            }
        }

        // Check for partial HTML entities at the split point
        let entity_start = search_range[..safe_max].rfind('&');
        if let Some(amp_pos) = entity_start {
            let after_amp = &search_range[amp_pos..safe_max];
            if !after_amp.contains(';') && after_amp.len() <= 8 {
                safe_max = amp_pos;
            }
        }

        // Existing logic: prefer paragraph boundary, then newline
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

    /// Spawn a background task that sends typing indicators every 4 seconds
    /// until the cancellation token is triggered.
    fn start_typing_indicator(&self) {
        let registry = self.channel_registry.clone();
        let channel_id = self.route.channel_id.clone();
        let conversation_id = self.route.conversation_id.clone();
        let cancel = self.typing_cancel.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(4));
            interval.tick().await; // Skip first immediate tick
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let _ = registry.send_typing(&channel_id, &conversation_id).await;
                    }
                    _ = cancel.cancelled() => {
                        break;
                    }
                }
            }
        });
    }

    /// React on the original inbound message (best-effort, non-blocking)
    async fn react_on_inbound(&self, emoji: &str) {
        if let Some(ref msg_id) = self.route.inbound_message_id {
            tracing::info!(
                target: "multimodal",
                probe = "P7_reaction",
                run_id = %self.run_id,
                emoji = %emoji,
                message_id = %msg_id.as_str(),
                "Processing status reaction"
            );
            let _ = self
                .channel_registry
                .react(
                    &self.route.channel_id,
                    &self.route.conversation_id,
                    msg_id,
                    emoji,
                )
                .await;
        }
    }
}

impl Drop for ReplyEmitter {
    fn drop(&mut self) {
        self.typing_cancel.cancel();
    }
}

#[async_trait]
impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match event {
            StreamEvent::ResponseChunk {
                content, is_final, is_intermediate, ..
            } => {
                if is_intermediate {
                    // Intermediate message: send immediately as a standalone message
                    if !content.is_empty() {
                        if self.should_voice().await {
                            self.send_as_voice(&content).await;
                        } else if self.config.stream_enabled && content.chars().count() > TYPEWRITER_CHARS_PER_STEP * 2 {
                            self.send_typewriter(&content).await;
                        } else {
                            self.send_to_channel(&content).await;
                        }
                    }
                    // Do NOT buffer — this is a separate message from the final response
                } else {
                    // React with 👀 on first non-intermediate chunk and start typing indicator
                    if !self.has_sent.load(Ordering::SeqCst) && !content.is_empty() {
                        self.react_on_inbound("👀").await;
                        self.start_typing_indicator();
                    }

                    // Existing behavior: accumulate text into buffer
                    if !content.is_empty() {
                        self.buffer.lock().await.push_str(&content);
                    }

                    // Instant mode: flush on final chunk
                    if !self.config.stream_enabled && is_final {
                        let mut buffer = self.buffer.lock().await;
                        if !buffer.is_empty() {
                            let text = std::mem::take(&mut *buffer);
                            drop(buffer);
                            if self.should_voice().await {
                                self.send_as_voice(&text).await;
                            } else {
                                self.send_to_channel(&text).await;
                            }
                        }
                    }
                    // Typewriter mode: do nothing here, wait for RunComplete
                }
            }

            StreamEvent::RunComplete { summary, .. } => {
                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Flush accumulated buffer (always flush — intermediate messages
                // may have set has_sent, but the buffer holds the final response)
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };

                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else if self.config.stream_enabled {
                        self.send_typewriter(&text).await;
                    } else {
                        self.send_to_channel(&text).await;
                    }
                }

                // Fallback: if buffer was empty (race with fire-and-forget emit),
                // use final_response from summary
                if text.is_empty() {
                    if let Some(ref final_response) = summary.final_response {
                        if !final_response.is_empty() {
                            debug!(
                                "Run {} complete, sending final_response as fallback (length: {})",
                                self.run_id,
                                final_response.len()
                            );
                            if self.should_voice().await {
                                self.send_as_voice(final_response).await;
                            } else if self.config.stream_enabled {
                                self.send_typewriter(final_response).await;
                            } else {
                                self.send_to_channel(final_response).await;
                            }
                        }
                    }
                }

                // React with 👍 on successful completion
                self.react_on_inbound("👍").await;
            }

            StreamEvent::RunError { error, .. } => {
                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Flush any partial response
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel(&text).await;
                    }
                }

                warn!("Run {} failed: {}", self.run_id, error);
                self.send_error(&error).await;

                // React with 👎 on error
                self.react_on_inbound("👎").await;
            }

            StreamEvent::AskUser { question, .. } => {
                // Flush buffer first
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel(&text).await;
                    }
                }

                if self.should_voice().await {
                    self.send_as_voice(&question).await;
                } else {
                    self.send_to_channel(&question).await;
                }
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
            voice_enabled: false,
            voice_reply_hint: false,
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

    #[test]
    fn test_split_message_preserves_code_block() {
        let code = "x".repeat(100);
        let content = format!("Before text\n\n```rust\n{}\n```\n\nAfter text", code);
        let chunks = ReplyEmitter::split_message(&content, 80);
        for chunk in &chunks {
            let count = chunk.matches("```").count();
            assert!(
                count % 2 == 0 || count == 0,
                "Chunk has unbalanced code fence: {:?}",
                chunk
            );
        }
    }

    #[test]
    fn test_split_message_html_entity_safe() {
        let content = format!("{}a&amp;b{}", "x".repeat(95), "y".repeat(100));
        let chunks = ReplyEmitter::split_message(&content, 100);
        for chunk in &chunks {
            assert!(
                !chunk.ends_with('&') && !chunk.ends_with("&a") && !chunk.ends_with("&am"),
                "Chunk ends with partial HTML entity: {:?}",
                chunk
            );
        }
    }

}

use crate::sync_primitives::Ordering;
use tracing::{debug, error, info, warn};

use super::super::sanitize::sanitize_llm_output;
use super::ReplyEmitter;
use crate::gateway::channel::OutboundMessage;

impl ReplyEmitter {
    /// Returns true when voice output should be attempted.
    /// Dynamically check if voice output should be used.
    ///
    /// Re-reads `VoiceState` from `ChannelRegistry` on every call so that
    /// mid-request `voice_mode_set` tool calls take effect immediately
    /// (e.g. the confirmation message itself is voiced).
    pub(crate) async fn should_voice(&self) -> bool {
        // Static hint: user sent an audio message. Deliberate product
        // behavior — voice input gets a voice reply even when the channel's
        // voice mode is off (send_as_voice still falls back to text when no
        // speech provider is available).
        if self.config.voice_reply_hint {
            debug!("should_voice=true (voice_reply_hint)");
            return true;
        }
        // Dynamic check: read current voice state from registry
        // Check both the specific channel and "default" (fallback when
        // voice_mode_set tool is called without explicit channel_id)
        let channel_id = self.route.channel_id.as_str();
        let live_state = self.channel_registry.get_voice_state(channel_id).await;
        debug!(
            "should_voice: channel={}, enabled={}",
            channel_id,
            live_state.is_active()
        );
        if live_state.is_active() {
            return true;
        }
        // Fallback: check "default" key (used when tool doesn't know channel)
        if channel_id != "default" {
            let default_state = self.channel_registry.get_voice_state("default").await;
            debug!(
                "should_voice: fallback 'default' enabled={}, has_gen_registry={}",
                default_state.is_active(),
                self.generation_registry.is_some()
            );
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
    pub(crate) async fn send_as_voice(&self, text: &str) {
        let sanitized = sanitize_llm_output(text);
        let text: &str = &sanitized;
        if text.is_empty() {
            return;
        }
        debug!(
            "send_as_voice called, text_len={}, has_gen_registry={}",
            text.len(),
            self.generation_registry.is_some()
        );
        if let Some(ref registry) = self.generation_registry {
            // Read live voice state from registry (not the stale local copy)
            // Check channel-specific first, then "default" fallback
            let channel_id = self.route.channel_id.as_str();
            // Track which channel's state we resolved to so the success/failure
            // bookkeeping below writes back to the SAME entry — the "default"
            // fallback must not credit the channel-specific entry.
            let mut state_channel_id = channel_id;
            let mut voice_state = self.channel_registry.get_voice_state(channel_id).await;
            if !voice_state.is_active() && channel_id != "default" {
                voice_state = self.channel_registry.get_voice_state("default").await;
                state_channel_id = "default";
            }
            let gen_config = match self.generation_config {
                Some(ref cfg) => cfg.read().await.clone(),
                None => {
                    self.send_to_channel(text).await;
                    return;
                }
            };

            use crate::gateway::voice::outbound::TtsOutcome;
            match crate::gateway::voice::outbound::generate_tts_outcome(
                text,
                &voice_state,
                registry,
                &gen_config,
            )
            .await
            {
                TtsOutcome::Generated(attachment) => {
                    // Reset the registry-tracked failure counter so subsequent
                    // runs start from 0 (the local per-emitter copy would
                    // otherwise reset on every new emitter and never persist).
                    voice_state.record_success();
                    self.channel_registry
                        .set_voice_state(state_channel_id, voice_state)
                        .await;

                    // Send voice-only message (no text — voice replaces text)
                    let message = OutboundMessage {
                        conversation_id: self.route.conversation_id.clone(),
                        text: String::new(),
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
                }
                TtsOutcome::Failed => {
                    // TTS generation failed — record on the registry-tracked
                    // state (not the local per-emitter copy) so 3 consecutive
                    // failures auto-disable the channel in the registry and
                    // subsequent runs fall back to text without re-attempting
                    // voice.
                    let auto_disabled = voice_state.record_failure();
                    self.channel_registry
                        .set_voice_state(state_channel_id, voice_state)
                        .await;
                    if auto_disabled {
                        warn!(
                            "Voice auto-disabled for channel {} after 3 consecutive TTS failures",
                            state_channel_id
                        );
                    }
                    // Fallback to plain text
                    self.send_to_channel(text).await;
                }
            }
        } else {
            // No generation registry — fall back to text
            self.send_to_channel(text).await;
        }

        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
    }

    /// Drain accumulated explicit reasoning text, returning None if empty.
    pub(crate) async fn take_reasoning_buffer(&self) -> Option<String> {
        let mut buf = self.reasoning_buffer.lock().await;
        let content = std::mem::take(&mut *buf);
        if content.is_empty() {
            None
        } else {
            Some(content)
        }
    }

    /// Drain pending media, download in parallel via `MediaCache`, return Attachments.
    pub(crate) async fn drain_and_send_media(&self) -> Vec<crate::gateway::channel::Attachment> {
        let pending_count = self.pending_media.lock().await.len();
        debug!(run_id = %self.run_id, pending_count = pending_count, "drain_and_send_media called");
        let media_items = std::mem::take(&mut *self.pending_media.lock().await);
        if media_items.is_empty() {
            return vec![];
        }

        info!(
            run_id = %self.run_id,
            count = media_items.len(),
            urls = ?media_items.iter().map(|i| &i.url).collect::<Vec<_>>(),
            "Draining pending media for download"
        );

        let attachments = futures::future::join_all(
            media_items
                .iter()
                .map(|item| self.media_cache.download_media_item(item, &self.run_id)),
        )
        .await;

        for att in &attachments {
            info!(
                run_id = %self.run_id,
                mime = %att.mime_type,
                has_path = att.path.is_some(),
                has_url = att.url.is_some(),
                size = ?att.size,
                "Media attachment ready"
            );
        }

        attachments
    }

    /// Send media as a separate standalone message.
    pub(crate) async fn send_media_standalone(
        &self,
        attachments: Vec<crate::gateway::channel::Attachment>,
    ) {
        if attachments.is_empty() {
            return;
        }
        let count = attachments.len();
        info!(
            run_id = %self.run_id,
            channel = %self.route.channel_id,
            count = count,
            "Sending media standalone message"
        );
        let message = OutboundMessage {
            conversation_id: self.route.conversation_id.clone(),
            text: String::new(),
            attachments,
            reply_to: None,
            inline_keyboard: None,
            metadata: Default::default(),
        };
        match self
            .channel_registry
            .send(&self.route.channel_id, message)
            .await
        {
            Ok(_) => {
                info!(run_id = %self.run_id, count = count, "Media standalone message sent successfully")
            }
            Err(e) => warn!(error = %e, "Failed to send media standalone message"),
        }
    }

    pub(crate) fn format_content(&self, content: &str, _is_first: bool) -> String {
        content.to_string()
    }

    // ── Shared helpers ──────────────────────────────────────────────────

    const MAX_MESSAGE_LENGTH: usize = 4000;

    /// Send content to the channel, sanitizing first and splitting into chunks.
    pub(crate) async fn send_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let content = sanitize_llm_output(content);
        self.send_to_channel_sanitized(&content, None).await;
    }

    /// Send content to the channel, extracting embedded reasoning, sanitizing,
    /// and splitting into chunks if too long.
    pub(crate) async fn send_to_channel_with_reasoning(
        &self,
        content: &str,
        reasoning: Option<&str>,
    ) {
        if content.is_empty() && reasoning.is_none_or(|r| r.is_empty()) {
            return;
        }

        let (embedded_reasoning, answer) = super::super::sanitize::split_reasoning(content);
        let final_reasoning = match (reasoning, embedded_reasoning) {
            (Some(r), Some(e)) if !r.is_empty() && !e.is_empty() => Some(format!("{r}\n\n{e}")),
            (Some(r), _) if !r.is_empty() => Some(r.to_string()),
            (None, Some(e)) => Some(e),
            _ => None,
        };

        let sanitized = sanitize_llm_output(&answer);
        self.send_to_channel_sanitized(&sanitized, final_reasoning.as_deref())
            .await;
    }

    /// Send pre-sanitized content to the channel, splitting into chunks if too long.
    ///
    /// Callers that have already called `sanitize_llm_output` should use this
    /// to avoid redundant sanitization passes.
    pub(crate) async fn send_to_channel_sanitized(&self, content: &str, reasoning: Option<&str>) {
        if content.is_empty() && reasoning.is_none_or(|r| r.is_empty()) {
            return;
        }

        let is_first_send = !self.has_sent.load(Ordering::SeqCst);
        self.has_sent.store(true, Ordering::SeqCst);

        let content = self.format_content(content, is_first_send);

        let chunks = Self::split_message(&content, Self::MAX_MESSAGE_LENGTH);
        let total_chunks = chunks.len();

        let metadata = reasoning
            .filter(|r| !r.is_empty())
            .map(|r| {
                let mut m = std::collections::HashMap::new();
                m.insert("reasoning".to_string(), r.to_string());
                m
            })
            .unwrap_or_default();

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
                metadata: metadata.clone(),
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

        let media_attachments = self.drain_and_send_media().await;
        self.send_media_standalone(media_attachments).await;
    }

    /// Split outbound content into channel-sized chunks.
    ///
    /// Thin delegate to the single canonical splitter
    /// ([`crate::gateway::formatter::MessageFormatter::split`]) so the streaming
    /// reply path and the outbound channel adapters share one fence-aware
    /// implementation. The canonical splitter handles UTF-8 boundaries,
    /// paragraph/line preference, and closing + re-opening fences that overflow
    /// `max_len`.
    pub(crate) fn split_message(content: &str, max_len: usize) -> Vec<String> {
        crate::gateway::formatter::MessageFormatter::split(content, max_len)
    }

    pub(crate) async fn send_error(&self, error: &str) {
        let error_message = format!("Error: {error}");
        self.send_to_channel(&error_message).await;
    }

    /// Spawn a background task that sends typing indicators every 4 seconds
    /// until the cancellation token is triggered.
    pub(crate) fn start_typing_indicator(&self) {
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
    pub(crate) async fn react_on_inbound(&self, emoji: &str) {
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

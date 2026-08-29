use crate::sync_primitives::Ordering;
use tracing::{debug, error, info, warn};

use super::super::sanitize::sanitize_llm_output;
use super::ReplyEmitter;
use crate::gateway::channel::OutboundMessage;

/// Whether a send is eligible for the side-answer badge.
///
/// `MayBeTheAnswer` does **not** mean "badge it" — `config.side_answer` and the
/// `answering` latch still decide. It means the call site does not rule it out.
/// `NeverTheAnswer` is the call site asserting that it knows this text is not
/// the run's answer, so the latch being open must not reach it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Marking {
    MayBeTheAnswer,
    NeverTheAnswer,
}

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

    /// Drain the run's pending media.
    ///
    /// A pure drain — no fetching. The attachments arrive already resolved from
    /// the tool-dispatch chokepoint (`tools::scoped::artifact_harvest`), which
    /// wrote them into this run's media session directory. This used to
    /// re-download every item here, which was both a second fetch of a URL the
    /// harvest had just fetched and the reason a failure was undiscoverable:
    /// every caller of this method runs at `RunComplete` / `RunError` /
    /// `AskUser`, i.e. after the loop has ended, so there was no turn left in
    /// which the model could be told.
    pub(crate) async fn drain_and_send_media(&self) -> Vec<crate::gateway::channel::Attachment> {
        let attachments = std::mem::take(&mut *self.pending_media.lock().await);
        if attachments.is_empty() {
            return vec![];
        }

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

    /// A run's closing media act: deliver whatever is left in the buffer, then
    /// drop the temp files the fetch created.
    ///
    /// Every emitter that owns text delivery itself still has to close this
    /// leg, and there are now three of them — this `ReplyEmitter`'s own
    /// `RunComplete`, Telegram's orchestrated emitter, and Feishu's streaming
    /// card. Each was reconstructing the same three steps, and each got a
    /// different subset right (the native-stream branch drained without
    /// cleaning up; the two channel emitters did neither). One method so the
    /// order is fixed in one place: the drain **must** precede the cleanup,
    /// which deletes the very files the attachments point at.
    ///
    /// Idempotent — the buffer is `mem::take`n, so a second call (or a run that
    /// produced no media at all) sends nothing.
    ///
    /// # Why the cleanup is conditional
    ///
    /// The delivery queue may have persisted the attachment message for durable
    /// retry, and when it could not inline the bytes (unreadable file, or a
    /// payload that would blow `max_payload_bytes` — a JSON byte array is ~4×
    /// the file) the queued row still references the **path**, into this very
    /// directory. Deleting it on the next statement turned a recoverable
    /// reconnect into a permanent loss that then dead-lettered as `Ambiguous`,
    /// which redrive refuses: the operator was told the outcome was unknown for
    /// a picture that provably never left the machine.
    ///
    /// `MediaCache::cleanup_stale()` at boot is still the backstop, so
    /// withholding the delete here leaks nothing permanently.
    ///
    /// The question is asked of [`ReplyEmitter::media_may_be_queued`], not of
    /// this call's own result: media leaves the emitter from four drain sites
    /// and only this one is followed by the cleanup, so a transient failure at
    /// any of the other three would otherwise have its files deleted by the
    /// empty drain that lands here.
    pub(crate) async fn deliver_run_media(&self) {
        let attachments = self.drain_and_send_media().await;
        self.send_media_standalone(attachments).await;
        if self.media_may_be_queued.load(Ordering::SeqCst) {
            warn!(
                run_id = %self.run_id,
                "media send failed transiently; leaving this run's media cache in place \
                 because a queued row may still reference it by path"
            );
            return;
        }
        if let Err(e) = crate::media::cache::MediaCache::cleanup_session(&self.run_id) {
            warn!(error = %e, "Failed to cleanup media session");
        }
    }

    /// Send media as a separate standalone message.
    ///
    /// Latches [`ReplyEmitter::media_may_be_queued`] when the send failed in a
    /// way the durable queue accepts (`delivery_queue::should_enqueue`) — i.e. a
    /// queued row for this media may now exist, and it may reference the files
    /// by path. Asking the queue's own predicate rather than re-deriving custody
    /// here keeps one answer to "is this recoverable?"; latching it on the
    /// emitter rather than returning it means the three call sites that never
    /// clean up inherit the answer without knowing the problem exists.
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
                info!(run_id = %self.run_id, count = count, "Media standalone message sent successfully");
            }
            Err(e) => {
                let queued = crate::gateway::delivery_queue::should_enqueue(&e);
                warn!(error = %e, queued, "Failed to send media standalone message");
                if queued {
                    self.media_may_be_queued.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    pub(crate) fn format_content(&self, content: &str, _is_first: bool) -> String {
        content.to_string()
    }

    /// Would [`ReplyEmitter::mark_side_answer`] change the text right now?
    ///
    /// Both halves matter. `config.side_answer` is fixed for the emitter's
    /// lifetime (see its doc); [`ReplyEmitter::answering`] is what keeps the
    /// marker off the progress messages a side question emits before its answer.
    ///
    /// Split out from `mark_side_answer` because one arm needs to know the
    /// answer *before* it has a string: `StreamAction::Done` means the last
    /// debounced edit already put the whole answer on screen, so the only way to
    /// badge it is to issue an edit that would otherwise not happen at all — and
    /// an unconditional one would change what every ordinary run puts on the
    /// wire.
    pub(crate) fn is_marking(&self) -> bool {
        self.config.side_answer && self.answering.load(Ordering::SeqCst)
    }

    /// Mark `text` as a side answer, if this run is one and is delivering its
    /// answer right now.
    ///
    /// Every caller passes text that has already been through
    /// `sanitize_llm_output` — the marker must not be visible to the sanitizer,
    /// which strips model-authored framing and would be reasoning about a
    /// prefix no model wrote.
    pub(crate) fn mark_side_answer<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.is_marking() {
            std::borrow::Cow::Owned(crate::gateway::btw::format_side_answer(text))
        } else {
            std::borrow::Cow::Borrowed(text)
        }
    }

    /// Open the [`ReplyEmitter::answering`] latch: text handed to the channel
    /// from here until [`ReplyEmitter::end_answering`] is the run's answer.
    pub(crate) fn begin_answering(&self) {
        self.answering.store(true, Ordering::SeqCst);
    }

    /// Close the latch. **Every [`ReplyEmitter::begin_answering`] pairs with
    /// one of these, in the same block.**
    ///
    /// There are two answer deliveries, not one: `RunComplete`, and the
    /// instant-mode flush on a final `ResponseChunk`. Each opens the latch,
    /// delivers, and closes it — so anything this emitter sends outside those
    /// two (a `RunError` that followed, a late media caption, whatever the next
    /// author adds) is not badged. Pairing locally rather than relying on a
    /// later caller to close is the point: an unpaired opener is an invariant
    /// about call order, which holds until someone changes the order.
    pub(crate) fn end_answering(&self) {
        self.answering.store(false, Ordering::SeqCst);
    }

    /// Whether this emitter is delivering a `/btw` side answer.
    ///
    /// Read by the two channel emitters that own text delivery themselves
    /// (Feishu's streaming card, Telegram's orchestrated lanes) and therefore
    /// never reach this emitter's own `RunComplete` arm. They hold a
    /// `ReplyEmitter` already — for the media leg — so they ask it rather than
    /// growing a second constructor argument that a future call site could
    /// forget to pass.
    #[must_use]
    pub(crate) fn is_side_answer(&self) -> bool {
        self.config.side_answer
    }

    // ── Shared helpers ──────────────────────────────────────────────────

    /// How long one outbound message may be, for THIS channel.
    ///
    /// Delegates to [`crate::gateway::channel::outbound_chunk_len`] — the one
    /// answer every outbound chunker shares — reading the cap
    /// `apply_channel_capabilities` already copied out of the channel's own
    /// `ChannelCapabilities`. This used to be a hardcoded 4000 regardless of
    /// the channel; see that function for what that cost.
    fn outbound_chunk_len(&self) -> usize {
        crate::gateway::channel::outbound_chunk_len(self.config.max_message_length)
    }

    /// Send content to the channel, sanitizing first and splitting into chunks.
    pub(crate) async fn send_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let content = sanitize_llm_output(content);
        self.send_to_channel_sanitized(&content, None).await;
    }

    /// Send text that travels *beside* the answer and is never the answer.
    ///
    /// The `🤔 <reasoning>` preview is the one such send, and it goes out
    /// **after** the latch opens — a side question whose provider emitted
    /// reasoning would otherwise get `💬 🤔 <chain of thought>` and then
    /// `💬 <answer>`: a badge on text that is not the answer, and two badges in
    /// one conversation for one side question, which is the interleaving the
    /// badge exists to disambiguate, inverted.
    ///
    /// Deliberately a separate entry point rather than "move the reasoning send
    /// above `begin_answering()`": the latch has to be open before it, because
    /// the native-stream branch delivers and returns earlier still.
    pub(crate) async fn send_aside_to_channel(&self, content: &str) {
        if content.is_empty() {
            return;
        }

        let content = sanitize_llm_output(content);
        self.deliver_to_channel(&content, None, Marking::NeverTheAnswer)
            .await;
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
        self.deliver_to_channel(content, reasoning, Marking::MayBeTheAnswer)
            .await;
    }

    /// THE outbound text chokepoint. Every non-streamed message this emitter
    /// sends passes here, already sanitized and not yet split.
    async fn deliver_to_channel(&self, content: &str, reasoning: Option<&str>, marking: Marking) {
        if content.is_empty() && reasoning.is_none_or(|r| r.is_empty()) {
            return;
        }

        let is_first_send = !self.has_sent.load(Ordering::SeqCst);
        self.has_sent.store(true, Ordering::SeqCst);

        let content = self.format_content(content, is_first_send);
        // Marking before the split puts the badge on the first chunk only, which
        // is what a reader scanning a conversation needs — a badge repeated on
        // every chunk of one long answer reads as several answers.
        let content = match marking {
            Marking::MayBeTheAnswer => self.mark_side_answer(&content).into_owned(),
            Marking::NeverTheAnswer => content,
        };

        let chunks = Self::split_message(&content, self.outbound_chunk_len());
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
                    // Whether the rest of the answer is still worth offering is
                    // the queue's question, so ask the queue's own predicate
                    // rather than writing a second classification here.
                    //
                    // Transient (`NotConnected` / `RateLimited`): keep going.
                    // Each remaining chunk fails live too and is persisted by
                    // `ChannelRegistry::maybe_enqueue` in ascending row id, and
                    // `claim_conversation` replays a conversation in id order —
                    // so the tail is delivered late but in one piece. Breaking
                    // here dropped chunks i+1..N on the floor: not sent, not
                    // queued, not dead-lettered, and not even named in this log
                    // line, so `channel_outbox` showed a healthy queue while the
                    // conversation ended mid-sentence forever.
                    let transient = crate::gateway::delivery_queue::should_enqueue(&e);
                    error!(
                        channel = %self.route.channel_id,
                        chunk = i + 1,
                        total_chunks,
                        abandoned = if transient { 0 } else { total_chunks - i - 1 },
                        error = %e,
                        "Failed to send reply chunk to channel"
                    );
                    if !transient {
                        // Terminal for this message: retrying the tail against a
                        // transport that just refused it permanently only adds
                        // noise. The count above is what makes the loss nameable.
                        break;
                    }
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

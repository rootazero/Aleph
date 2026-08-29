use super::{sanitize_llm_output, split_reasoning, ReplyEmitter, ReplyEmitterConfig};
use crate::gateway::channel::{ChannelId, ConversationId};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::event_emitter::EventEmitter;
use crate::gateway::inbound_context::ReplyRoute;
use crate::sync_primitives::Arc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{
        Channel, ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState, ChannelStatus,
        MessageId, OutboundMessage, SendResult, StreamProtocol,
    };
    use crate::gateway::media::PendingMedia;

    /// Captures every `OutboundMessage` the registry hands it, so a test can
    /// assert what actually left for the chat rather than only what the emitter
    /// took off its own buffer.
    struct RecordingChannel {
        info: ChannelInfo,
        state: ChannelState,
        sent: Arc<tokio::sync::Mutex<Vec<OutboundMessage>>>,
    }

    #[async_trait::async_trait]
    impl Channel for RecordingChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
            self.sent.lock().await.push(message);
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    /// Emitter wired to a recording channel; returns the emitter plus the log.
    async fn emitter_over_recorder(
        run_id: &str,
        pending: PendingMedia,
    ) -> (ReplyEmitter, Arc<tokio::sync::Mutex<Vec<OutboundMessage>>>) {
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let registry = ChannelRegistry::new();
        registry
            .register(Box::new(RecordingChannel {
                info: ChannelInfo {
                    id: ChannelId::new("rec"),
                    name: "rec".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: Default::default(),
                },
                state: ChannelState::new(8),
                sent: sent.clone(),
            }))
            .await;
        let emitter = ReplyEmitter::new(
            Arc::new(registry),
            ReplyRoute::new(ChannelId::new("rec"), ConversationId::new("conv-1")),
            run_id.to_string(),
            pending,
        );
        (emitter, sent)
    }

    /// One buffer entry, already resolved — which is what the delivery buffer
    /// now holds. The emitter no longer fetches anything; the tool-dispatch
    /// chokepoint did that while the model could still hear about a failure.
    fn one_inline_image() -> PendingMedia {
        Arc::new(tokio::sync::Mutex::new(vec![
            crate::gateway::media::resolved_test_attachment(),
        ]))
    }

    /// The single source every run-end path now shares: what is in the buffer
    /// leaves as an attachment message, and the buffer is left empty.
    #[tokio::test]
    async fn deliver_run_media_sends_the_buffer_as_attachments() {
        let pending = one_inline_image();
        let (emitter, sent) = emitter_over_recorder("run-media", pending.clone()).await;

        emitter.deliver_run_media().await;

        let sent = sent.lock().await;
        assert_eq!(sent.len(), 1, "one standalone attachment message");
        assert_eq!(sent[0].attachments.len(), 1);
        assert_eq!(sent[0].attachments[0].mime_type, "image/png");
        assert!(
            sent[0].text.is_empty(),
            "media rides its own message, it must not re-post text"
        );
        assert!(pending.lock().await.is_empty(), "buffer drained");
    }

    /// Idempotent by construction (`mem::take`) — the run-end paths call this
    /// after earlier mid-run drains, and a second call must not re-post.
    #[tokio::test]
    async fn deliver_run_media_is_idempotent() {
        let (emitter, sent) = emitter_over_recorder("run-twice", one_inline_image()).await;

        emitter.deliver_run_media().await;
        emitter.deliver_run_media().await;

        assert_eq!(sent.lock().await.len(), 1);
    }

    /// A run that produced no media must stay silent — no empty message.
    #[tokio::test]
    async fn deliver_run_media_without_media_sends_nothing() {
        let (emitter, sent) = emitter_over_recorder("run-quiet", PendingMedia::default()).await;

        emitter.deliver_run_media().await;

        assert!(sent.lock().await.is_empty());
    }

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

    fn caps_of(editing: bool, protocol: StreamProtocol) -> ChannelCapabilities {
        ChannelCapabilities {
            editing,
            stream_protocol: protocol,
            max_message_length: 4096,
            ..ChannelCapabilities::default()
        }
    }

    /// The bug this floor exists for: a channel that cannot edit received the
    /// first flush and nothing else, because `Channel::edit`'s default body is
    /// an `Err` the streaming emitter drops with no send fallback.
    #[test]
    fn a_channel_that_cannot_edit_never_streams() {
        for protocol in [
            StreamProtocol::None,
            StreamProtocol::EditBased,
            StreamProtocol::Native,
        ] {
            let mut config = ReplyEmitterConfig::from_output_mode("typewriter");
            config.apply_channel_capabilities(&caps_of(false, protocol));
            assert!(
                !config.stream_enabled,
                "{protocol:?} + editing:false must not stream"
            );
        }
    }

    /// `line` and `wechat` declared `EditBased` while refusing to edit. A floor
    /// written as the `else` of the widening arm would never have run for them.
    #[test]
    fn the_floor_outranks_the_edit_based_widening() {
        let mut config = ReplyEmitterConfig::from_output_mode("instant");
        config.apply_channel_capabilities(&caps_of(false, StreamProtocol::EditBased));
        assert!(!config.stream_enabled);
    }

    /// Narrowing only: a channel that *can* edit keeps exactly the behaviour it
    /// had before the floor existed — including `slack`/`mattermost`/`msteams`,
    /// which stream while declaring `StreamProtocol::None`.
    ///
    /// The cap is copied on **both** protocols. It used to be set only inside
    /// the `EditBased` arm — this test previously asserted `0` for `None` with
    /// the message "None does not set a cap", which encoded exactly the defect:
    /// a length limit is a fact about the transport, not about streaming, so
    /// every `StreamProtocol::None` channel (slack 3000, irc 400, …) left it at
    /// 0 and both consumers of it were silently disabled.
    #[test]
    fn a_channel_that_can_edit_keeps_the_global_preference() {
        let mut typewriter = ReplyEmitterConfig::from_output_mode("typewriter");
        typewriter.apply_channel_capabilities(&caps_of(true, StreamProtocol::None));
        assert!(typewriter.stream_enabled);
        assert_eq!(
            typewriter.max_message_length, 4096,
            "the declared cap is a transport fact and reaches the chunker on every protocol"
        );

        let mut instant = ReplyEmitterConfig::from_output_mode("instant");
        instant.apply_channel_capabilities(&caps_of(true, StreamProtocol::EditBased));
        assert!(instant.stream_enabled, "EditBased still widens");
        assert_eq!(instant.max_message_length, 4096);
    }
    #[test]
    fn test_reply_route() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route.clone(),
            "run-123".to_string(),
            PendingMedia::default(),
        );
        assert_eq!(emitter.run_id(), "run-123");
        assert_eq!(emitter.route().channel_id.as_str(), "imessage");
        assert_eq!(emitter.route().conversation_id.as_str(), "+15551234567");
    }
    #[test]
    fn test_custom_config() {
        let route = ReplyRoute::new(ChannelId::new("telegram"), ConversationId::new("12345"));
        let config = ReplyEmitterConfig {
            buffer_threshold: 1000,
            stream_enabled: true,
            voice_enabled: false,
            voice_reply_hint: false,
            debounce_ms: 800,
            min_initial_chars: 30,
            max_message_length: 4096,
            ..ReplyEmitterConfig::default()
        };
        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::with_config(
            registry,
            route,
            "run-456".to_string(),
            config,
            PendingMedia::default(),
        );
        assert_eq!(emitter.config.buffer_threshold, 1000);
        assert!(emitter.config.stream_enabled);
    }
    #[tokio::test]
    async fn test_sequence_counter() {
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("conv-1"));
        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route,
            "run-789".to_string(),
            PendingMedia::default(),
        );
        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
        assert_eq!(emitter.next_seq(), 2);
    }
    #[tokio::test]
    async fn test_buffer_accumulation() {
        let route = ReplyRoute::new(ChannelId::new("test"), ConversationId::new("conv-1"));
        let registry = Arc::new(ChannelRegistry::new());
        let emitter = ReplyEmitter::new(
            registry,
            route,
            "run-test".to_string(),
            PendingMedia::default(),
        );
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
    // ── sanitize_llm_output tests ──────────────────────────────────────

    /// The live stream and the terminal answer encode ONE contract in two
    /// lists. They drifted: `memory-context` was discarded from the stream and
    /// kept in the final answer, so a model that echoed the recall fence had it
    /// scrubbed from the live bubble and then written back over it by the
    /// Panel's `finalize_answer` — and posted to Telegram / Slack / the group
    /// transcript / cron results.
    ///
    /// Asserts the effect at the consumer (the span is gone from the output),
    /// not that a name appears in a list, so a tag added to `DISCARD_TAG_PAIRS`
    /// with no terminal counterpart fails here rather than leaking silently.
    #[test]
    fn discard_tag_pairs_are_all_stripped_from_the_terminal_answer() {
        for (open, close) in crate::memory::streaming_scrubber::DISCARD_TAG_PAIRS {
            let text = format!("Before. {open}SECRET{close} After.");
            let cleaned = sanitize_llm_output(&text);
            assert!(
                !cleaned.contains("SECRET"),
                "{open} is discarded from the live stream but survives into the \
                 final answer — the terminal surfaces would show what the stream \
                 already hid. Got: {cleaned}"
            );
            assert!(
                !cleaned.contains(*open) && !cleaned.contains(*close),
                "{open} fence itself must not reach the user. Got: {cleaned}"
            );
        }
    }

    /// The fence must survive inside code — a user asking *about* the tag is
    /// not the model echoing it.
    #[test]
    fn a_memory_fence_inside_code_is_left_alone() {
        let text = "Use `<memory-context>` to wrap recalled memory.";
        assert!(sanitize_llm_output(text).contains("memory-context"));
    }

    #[test]
    fn sanitize_no_tags_returns_borrowed() {
        let input = "Hello, this is a normal response.";
        let result = sanitize_llm_output(input);
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*result, input);
    }
    #[test]
    fn sanitize_strips_think_block() {
        let input = "<think>\nLet me analyze this...\n</think>\nThe answer is 42.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "The answer is 42.");
    }
    #[test]
    fn sanitize_strips_completion_check() {
        let input = "Here is the result.\n<completion-check>\n• Task done\n</completion-check>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here is the result.");
    }
    #[test]
    fn sanitize_strips_task_complete() {
        let input = "Done.\n<task-complete/>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }
    #[test]
    fn sanitize_strips_all_tags_combined() {
        let input = "<think>\nthinking...\n</think>\nHere is the answer.\n<completion-check>\n• ok\n</completion-check>\n<task-complete/>";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here is the answer.");
    }
    #[test]
    fn sanitize_collapses_blank_lines() {
        let input = "<think>x</think>\n\n\n\nActual response.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Actual response.");
    }
    #[test]
    fn sanitize_self_closing_with_space() {
        let input = "Done.\n<task-complete />";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }
    // ── Code-block awareness tests ────────────────────────────────────
    #[test]
    fn sanitize_preserves_think_in_fenced_code() {
        let input = "Example:\n```\n<think>code here</think>\n```\nDone.";
        let result = sanitize_llm_output(input);
        assert!(
            result.contains("<think>code here</think>"),
            "think tag inside fenced code should be preserved, got: {}",
            result
        );
    }
    #[test]
    fn sanitize_preserves_think_in_inline_code() {
        let input = "Use `<think>` tags for reasoning. The answer is 42.";
        let result = sanitize_llm_output(input);
        assert!(
            result.contains("<think>"),
            "think tag inside inline code should be preserved, got: {}",
            result
        );
    }
    #[test]
    fn sanitize_strips_think_outside_but_preserves_inside_code() {
        let input =
            "<think>reasoning</think>\nHere is code:\n```\n<think>example</think>\n```\nDone.";
        let result = sanitize_llm_output(input);
        assert!(
            !result.starts_with("<think>"),
            "think outside code should be stripped"
        );
        assert!(
            result.contains("<think>example</think>"),
            "think inside code should be preserved, got: {}",
            result
        );
    }
    // ── Multi-format thinking tag tests ───────────────────────────────
    #[test]
    fn sanitize_strips_thinking_tag() {
        let input = "<thinking>\nAnalyzing...\n</thinking>\nResult here.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Result here.");
    }
    #[test]
    fn sanitize_strips_thought_tag() {
        let input = "<thought>internal</thought>Answer.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Answer.");
    }
    #[test]
    fn sanitize_strips_antthinking_tag() {
        let input = "<antthinking>\nplanning...\n</antthinking>\nHere you go.";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Here you go.");
    }
    // ── Trailing incomplete directive tests ────────────────────────────
    #[test]
    fn sanitize_strips_trailing_brackets() {
        let input = "The answer is 42.[[";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "The answer is 42.");
    }
    #[test]
    fn sanitize_strips_trailing_incomplete_tag() {
        let input = "Done.\n<completion-check";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "Done.");
    }
    #[test]
    fn sanitize_preserves_less_than_in_text() {
        // "< 10" should NOT be stripped — it's math, not a tag
        let input = "if x < 10 then stop";
        let result = sanitize_llm_output(input);
        assert_eq!(&*result, "if x < 10 then stop");
    }
    // ── split_reasoning tests ─────────────────────────────────────────
    #[test]
    fn split_reasoning_no_tags() {
        let input = "Hello, world!";
        let (reasoning, answer) = split_reasoning(input);
        assert!(reasoning.is_none());
        assert_eq!(answer, "Hello, world!");
    }
    #[test]
    fn split_reasoning_extracts_think_block() {
        let input = "<think>Let me think...</think>\nAnswer is 42.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("Let me think..."));
        assert_eq!(answer, "\nAnswer is 42.");
    }
    #[test]
    fn split_reasoning_extracts_multiple_thinking_tags() {
        let input = "<think>first</think>\n<thinking>second</thinking>\nFinal.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("first\n\nsecond"));
        assert_eq!(answer, "\n\nFinal.");
    }
    #[test]
    fn split_reasoning_preserves_think_in_code() {
        let input = "<think>reasoning</think>\n```\n<think>code</think>\n```\nDone.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("reasoning"));
        assert!(answer.contains("<think>code</think>"));
    }
    #[test]
    fn split_reasoning_case_insensitive() {
        let input = "<THINK>caps</THINK>Answer.";
        let (reasoning, answer) = split_reasoning(input);
        assert_eq!(reasoning.as_deref(), Some("caps"));
        assert_eq!(answer, "Answer.");
    }
}

/// Guards for the outbound text chokepoint: how long a chunk may be, and what
/// happens to the rest of the answer when one chunk fails.
#[cfg(test)]
mod outbound_chunking_tests {
    use super::*;
    use crate::gateway::channel::{
        Channel, ChannelCapabilities, ChannelError, ChannelInfo, ChannelResult, ChannelState,
        ChannelStatus, MessageId, OutboundMessage, SendResult, StreamProtocol,
    };
    use crate::gateway::media::PendingMedia;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Records every attempt, and starts refusing once `attempts >= fail_from`.
    /// Recording the ATTEMPT (not just the successes) is the point: the defect
    /// guarded here is a chunk that was never offered to any transport at all.
    struct FlakyChannel {
        info: ChannelInfo,
        state: ChannelState,
        attempts: Arc<AtomicUsize>,
        seen: Arc<tokio::sync::Mutex<Vec<String>>>,
        fail_from: usize,
        err_from: fn(String) -> ChannelError,
    }

    #[async_trait::async_trait]
    impl Channel for FlakyChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().await.push(message.text.clone());
            if n + 1 >= self.fail_from {
                return Err((self.err_from)("stub".to_string()));
            }
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        }
    }

    struct Rig {
        emitter: ReplyEmitter,
        attempts: Arc<AtomicUsize>,
        seen: Arc<tokio::sync::Mutex<Vec<String>>>,
    }

    async fn rig(cap: usize, fail_from: usize, err_from: fn(String) -> ChannelError) -> Rig {
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let registry = ChannelRegistry::new();
        registry
            .register(Box::new(FlakyChannel {
                info: ChannelInfo {
                    id: ChannelId::new("rec"),
                    name: "rec".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: Default::default(),
                },
                state: ChannelState::new(8),
                attempts: attempts.clone(),
                seen: seen.clone(),
                fail_from,
                err_from,
            }))
            .await;

        // The cap arrives the way production supplies it: through the channel's
        // own declared capabilities, not as a literal on the emitter.
        let mut config = ReplyEmitterConfig::default();
        config.apply_channel_capabilities(&ChannelCapabilities {
            editing: false,
            stream_protocol: StreamProtocol::None,
            max_message_length: cap,
            ..ChannelCapabilities::default()
        });

        let emitter = ReplyEmitter::with_config(
            Arc::new(registry),
            ReplyRoute::new(ChannelId::new("rec"), ConversationId::new("conv-1")),
            "run-chunk".to_string(),
            config,
            PendingMedia::default(),
        );
        Rig {
            emitter,
            attempts,
            seen,
        }
    }

    /// Discord declares 2000 and its own `Channel::send` does no splitting, so
    /// a chunk over the cap is rejected outright and the whole answer is lost
    /// (`SendFailed` is not queueable). The chunker used to hold a hardcoded
    /// 4000 — a second, wrong answer to a question `ChannelCapabilities`
    /// already answers.
    #[tokio::test]
    async fn the_chunker_honours_the_channels_declared_cap() {
        let r = rig(2000, usize::MAX, ChannelError::SendFailed).await;
        r.emitter
            .send_to_channel_sanitized(&"a".repeat(3000), None)
            .await;

        let seen = r.seen.lock().await;
        assert!(seen.len() >= 2, "3000 bytes must not ride in one 2000 cap");
        for (i, text) in seen.iter().enumerate() {
            assert!(
                text.len() <= 2000,
                "chunk {i} is {} bytes, over the channel's declared 2000 cap — the \
                 transport rejects it and the whole answer is lost",
                text.len()
            );
        }
    }

    /// A channel that declares no cap keeps the historical fallback rather than
    /// handing one unbounded frame to the transport.
    #[tokio::test]
    async fn an_undeclared_cap_falls_back_instead_of_sending_one_huge_frame() {
        let r = rig(0, usize::MAX, ChannelError::SendFailed).await;
        r.emitter
            .send_to_channel_sanitized(&"a".repeat(9000), None)
            .await;
        let seen = r.seen.lock().await;
        for text in seen.iter() {
            assert!(text.len() <= 4000, "{} bytes", text.len());
        }
    }

    fn three_chunks() -> String {
        format!(
            "{}\n\n{}\n\n{}",
            "a".repeat(90),
            "b".repeat(90),
            "c".repeat(90)
        )
    }

    /// The tail used to be dropped on the floor: chunk `i` is persisted by
    /// `maybe_enqueue`, but `i+1..N` were never handed to anything — not sent,
    /// not queued, not dead-lettered, not even named in the log — so the
    /// operator saw one queued row and a healthy queue while the conversation
    /// ended mid-sentence forever.
    #[tokio::test]
    async fn a_transient_failure_midway_still_offers_every_remaining_chunk() {
        let r = rig(100, 2, ChannelError::NotConnected).await;
        r.emitter
            .send_to_channel_sanitized(&three_chunks(), None)
            .await;

        assert_eq!(
            r.attempts.load(Ordering::SeqCst),
            3,
            "every chunk must reach ChannelRegistry::send so the durable queue can \
             persist it; stopping at the failure discards the rest of the answer"
        );
        let seen = r.seen.lock().await;
        assert!(
            seen[1].starts_with('b') && seen[2].starts_with('c'),
            "{seen:?}"
        );
    }

    /// The other arm: a terminal error is not queueable, so retrying the tail
    /// against a transport that just refused permanently only adds noise. It
    /// still stops — the fix is a split on the queue's own predicate, not an
    /// unconditional `continue`.
    #[tokio::test]
    async fn a_terminal_failure_midway_still_stops() {
        let r = rig(100, 2, ChannelError::SendFailed).await;
        r.emitter
            .send_to_channel_sanitized(&three_chunks(), None)
            .await;

        assert_eq!(
            r.attempts.load(Ordering::SeqCst),
            2,
            "a permanent refusal must not be retried chunk by chunk"
        );
    }
}

/// Guard for the run-end media cleanup: it must not delete files a queued row
/// still references by path.
#[cfg(test)]
mod media_cleanup_tests {
    use super::*;
    use crate::gateway::channel::{
        Attachment, Channel, ChannelError, ChannelInfo, ChannelResult, ChannelState, ChannelStatus,
        MessageId, OutboundMessage, SendResult,
    };
    use crate::media::cache::MediaCache;

    struct StubChannel {
        info: ChannelInfo,
        state: ChannelState,
        err: fn(String) -> ChannelError,
        ok: bool,
    }

    #[async_trait::async_trait]
    impl Channel for StubChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            if self.ok {
                return Ok(SendResult {
                    message_id: MessageId::new("ok"),
                    timestamp: chrono::Utc::now(),
                });
            }
            Err((self.err)("stub".to_string()))
        }
    }

    /// Cache one real file under this run's media session and hand the emitter
    /// an attachment pointing at it, exactly as the harvest does.
    async fn emitter_with_cached_file(
        run_id: &str,
        ok: bool,
        err: fn(String) -> ChannelError,
    ) -> (ReplyEmitter, std::path::PathBuf) {
        let cached = MediaCache::new()
            .resolve(
                &Attachment {
                    id: "guard".to_string(),
                    mime_type: "image/png".to_string(),
                    filename: Some("shot.png".to_string()),
                    size: Some(5),
                    url: None,
                    path: None,
                    data: Some(b"hello".to_vec()),
                },
                run_id,
            )
            .await
            .expect("cache a file under this run's media session");
        let path = cached.local_path.clone();
        assert!(path.exists(), "fixture must actually create the file");

        let registry = ChannelRegistry::new();
        registry
            .register(Box::new(StubChannel {
                info: ChannelInfo {
                    id: ChannelId::new("rec"),
                    name: "rec".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: Default::default(),
                },
                state: ChannelState::new(8),
                err,
                ok,
            }))
            .await;

        // A path-backed attachment — the shape `take_media_custody` may refuse
        // to inline, leaving the queued row referencing this very file.
        let pending: crate::gateway::media::PendingMedia =
            Arc::new(tokio::sync::Mutex::new(vec![Attachment {
                id: "guard".to_string(),
                mime_type: "image/png".to_string(),
                filename: Some("shot.png".to_string()),
                size: Some(5),
                url: None,
                path: Some(path.to_string_lossy().into_owned()),
                data: None,
            }]));

        let emitter = ReplyEmitter::new(
            Arc::new(registry),
            ReplyRoute::new(ChannelId::new("rec"), ConversationId::new("conv-1")),
            run_id.to_string(),
            pending,
        );
        (emitter, path)
    }

    /// The defect: `deliver_run_media` deleted this run's whole media cache on
    /// the statement after the send, so a row queued with a bare `path` pointed
    /// at a file that was already gone — and it then dead-lettered as
    /// `Ambiguous`, which redrive refuses. "A path that might still be there"
    /// was false in-process, not merely across restarts.
    #[tokio::test]
    async fn a_queued_media_send_leaves_the_runs_media_cache_in_place() {
        let (emitter, path) =
            emitter_with_cached_file("run-media-queued", false, ChannelError::NotConnected).await;

        emitter.deliver_run_media().await;

        let survived = path.exists();
        let _ = MediaCache::cleanup_session("run-media-queued");
        assert!(
            survived,
            "the durable queue accepted this failure, so a queued row may reference {} \
             by path; deleting it makes the retry unrecoverable",
            path.display()
        );
    }

    /// The control that keeps the fix from degenerating into "never clean up":
    /// a delivered run still drops its temp files.
    #[tokio::test]
    async fn a_delivered_media_send_still_cleans_up() {
        let (emitter, path) =
            emitter_with_cached_file("run-media-delivered", true, ChannelError::NotConnected).await;

        emitter.deliver_run_media().await;

        assert!(
            !path.exists(),
            "nothing references {} any more; withholding the delete would leak",
            path.display()
        );
    }

    /// The cross-drain half. Media leaves this emitter from four sites and only
    /// `deliver_run_media` is followed by the cleanup, so gating on *this*
    /// call's own result protected only the last drain: a transient failure at
    /// the streaming-settle / `AskUser` / reply-tail drains left an empty
    /// buffer here, which "succeeded", and the files were deleted anyway.
    #[tokio::test]
    async fn a_queued_failure_at_an_earlier_drain_still_protects_the_cache() {
        let (emitter, path) =
            emitter_with_cached_file("run-media-earlier", false, ChannelError::NotConnected).await;

        // Exactly what the other three drain sites do.
        let attachments = emitter.drain_and_send_media().await;
        assert_eq!(
            attachments.len(),
            1,
            "fixture must actually hand this drain something to lose"
        );
        emitter.send_media_standalone(attachments).await;

        // …and then the run ends with nothing left to drain.
        emitter.deliver_run_media().await;

        let survived = path.exists();
        let _ = MediaCache::cleanup_session("run-media-earlier");
        assert!(
            survived,
            "an earlier drain's failure was queued, so {} may still be referenced by \
             a queued row; the empty final drain must not delete it",
            path.display()
        );
    }

    /// And a terminal failure is not queueable, so there is no row to protect.
    #[tokio::test]
    async fn a_terminally_failed_media_send_still_cleans_up() {
        let (emitter, path) =
            emitter_with_cached_file("run-media-terminal", false, ChannelError::SendFailed).await;

        emitter.deliver_run_media().await;

        assert!(!path.exists(), "{} should have been cleaned", path.display());
    }
}

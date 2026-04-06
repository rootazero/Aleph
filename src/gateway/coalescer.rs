//! Message Coalescer
//!
//! Merges rapid-fire inbound messages from the same conversation (or media group)
//! into a single `InboundMessage` before handing off to the agent loop.
//!
//! # Design
//!
//! - **DashMap** for lock-free concurrent buffer access
//! - **Debounce** window resets on each new message (configurable)
//! - **Early flush** when text ends with sentence-ending punctuation
//! - **Media group** aggregation keyed by Telegram `media_group_id`
//! - **Safety limits** on fragment count and total byte size

use std::time::{Duration, Instant};

use dashmap::DashMap;

use super::channel::InboundMessage;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Tuning knobs for the coalescing window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoalescingConfig {
    /// Default debounce window for regular messages (ms).
    pub debounce_ms: u64,
    /// Shortened deadline when a sentence-ending character is detected (ms).
    pub early_flush_ms: u64,
    /// Deadline for media-group album collection (ms).
    pub media_group_timeout_ms: u64,
    /// Maximum number of fragments per buffer before forced flush.
    pub max_fragments: usize,
    /// Maximum cumulative text bytes before forced flush.
    pub max_bytes: usize,
}

impl Default for CoalescingConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 800,
            early_flush_ms: 200,
            media_group_timeout_ms: 500,
            max_fragments: 12,
            max_bytes: 50_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal buffer
// ---------------------------------------------------------------------------

struct CoalesceBuffer {
    messages: Vec<InboundMessage>,
    deadline: Instant,
    total_text_bytes: usize,
    /// Non-None when this buffer aggregates a Telegram media group.
    /// Retained for future diagnostics / logging.
    #[allow(dead_code)]
    media_group_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Concurrent message coalescer.
///
/// Call [`push`] for every inbound message, then periodically call [`tick`]
/// (e.g. every 50-100 ms) to collect merged results.
pub struct MessageCoalescer {
    buffers: DashMap<String, CoalesceBuffer>,
    config: CoalescingConfig,
}

impl MessageCoalescer {
    pub fn new(config: CoalescingConfig) -> Self {
        Self {
            buffers: DashMap::new(),
            config,
        }
    }

    /// Ingest a message. Returns any immediately-flushed results when a buffer
    /// exceeds safety limits (max_fragments / max_bytes).
    pub fn push(&self, msg: InboundMessage) -> Vec<InboundMessage> {
        let now = Instant::now();
        let conversation_id = msg.conversation_id.as_str().to_owned();

        // Choose the buffer key: media_group_id takes precedence.
        let is_media_group = msg.meta_media_group_id().is_some();
        let key = if let Some(mg) = msg.meta_media_group_id() {
            format!("mg:{}", mg)
        } else {
            format!("cv:{}", msg.conversation_id.as_str())
        };

        let text_len = msg.text.len();

        // Determine the base deadline for this message.
        let base_deadline = if is_media_group {
            now + Duration::from_millis(self.config.media_group_timeout_ms)
        } else if is_sentence_end(&msg.text) {
            now + Duration::from_millis(self.config.early_flush_ms)
        } else {
            now + Duration::from_millis(self.config.debounce_ms)
        };

        let media_group_id = msg.meta_media_group_id().map(|s| s.to_owned());

        let mut flushed = Vec::new();

        // Insert or update the buffer.
        let mut entry = self.buffers.entry(key).or_insert_with(|| CoalesceBuffer {
            messages: Vec::new(),
            deadline: base_deadline,
            total_text_bytes: 0,
            media_group_id: media_group_id.clone(),
        });

        let buf = entry.value_mut();
        buf.messages.push(msg);
        buf.total_text_bytes += text_len;
        buf.deadline = base_deadline;

        tracing::debug!(
            conversation_id = %conversation_id,
            fragment_count = buf.messages.len(),
            total_text_bytes = buf.total_text_bytes,
            "message entered coalescing buffer",
        );

        // Check safety limits — flush everything accumulated so far.
        if buf.messages.len() >= self.config.max_fragments
            || buf.total_text_bytes >= self.config.max_bytes
        {
            let drained = std::mem::take(&mut buf.messages);
            buf.total_text_bytes = 0;
            flushed.push(Self::merge(drained));
        }

        // Drop the entry ref before potential remove (not needed here, but clear intent).
        drop(entry);

        flushed
    }

    /// Collect all expired buffers, merge each into a single `InboundMessage`.
    pub fn tick(&self) -> Vec<InboundMessage> {
        let now = Instant::now();
        let mut expired_keys = Vec::new();

        // First pass: find expired keys.
        for entry in self.buffers.iter() {
            if entry.value().deadline <= now {
                expired_keys.push(entry.key().clone());
            }
        }

        // Second pass: remove and merge.
        let mut results = Vec::with_capacity(expired_keys.len());
        for key in expired_keys {
            if let Some((_, buf)) = self.buffers.remove(&key) {
                if !buf.messages.is_empty() {
                    let fragment_count = buf.messages.len();
                    let conversation_id = buf
                        .messages
                        .first()
                        .map(|m| m.conversation_id.as_str())
                        .unwrap_or("unknown");
                    tracing::info!(
                        conversation_id = %conversation_id,
                        fragment_count = fragment_count,
                        "coalesced message flush on tick",
                    );
                    results.push(Self::merge(buf.messages));
                }
            }
        }

        results
    }

    /// Drain every buffer regardless of deadline and return merged messages.
    pub fn flush_all(&self) -> Vec<InboundMessage> {
        let keys: Vec<String> = self.buffers.iter().map(|e| e.key().clone()).collect();
        if !keys.is_empty() {
            tracing::info!(
                pending_buffers = keys.len(),
                "flush_all: draining all pending coalescing buffers",
            );
        }
        let mut results = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some((_, buf)) = self.buffers.remove(&key) {
                if !buf.messages.is_empty() {
                    results.push(Self::merge(buf.messages));
                }
            }
        }
        results
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Merge a vec of messages into one.
    ///
    /// - Text: concatenated with "\n"
    /// - Attachments / metadata: collected from all fragments
    /// - ID: lexicographically largest message id
    /// - Timestamp: from the first (earliest) message
    fn merge(mut messages: Vec<InboundMessage>) -> InboundMessage {
        debug_assert!(!messages.is_empty(), "merge called with empty vec");

        if messages.len() == 1 {
            return messages.remove(0);
        }

        // Take the first message as the base.
        let mut base = messages.remove(0);

        let mut texts: Vec<String> = Vec::with_capacity(messages.len() + 1);
        if !base.text.is_empty() {
            texts.push(std::mem::take(&mut base.text));
        }

        let mut max_id = base.id.as_str().to_owned();

        for msg in messages {
            if !msg.text.is_empty() {
                texts.push(msg.text);
            }
            base.attachments.extend(msg.attachments);
            base.metadata.extend(msg.metadata);

            if msg.id.as_str() > max_id.as_str() {
                max_id = msg.id.as_str().to_owned();
            }
        }

        base.text = texts.join("\n");
        base.id = super::channel::MessageId::new(max_id);

        base
    }
}

/// Returns `true` when the trimmed text ends with a sentence-ending character.
fn is_sentence_end(text: &str) -> bool {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    matches!(trimmed.chars().last(), Some('?' | '。' | '！' | '.' | '？'))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{
        Attachment, ChannelId, ConversationId, MessageId, MessageMeta, UserId,
    };
    use chrono::Utc;

    /// Helper to build a minimal InboundMessage.
    fn make_msg(id: &str, conv: &str, text: &str) -> InboundMessage {
        InboundMessage {
            id: MessageId::new(id),
            channel_id: ChannelId::new("tg"),
            conversation_id: ConversationId::new(conv),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        }
    }

    fn make_media_msg(id: &str, conv: &str, group: &str, text: &str) -> InboundMessage {
        let mut msg = make_msg(id, conv, text);
        msg.metadata
            .push(MessageMeta::MediaGroupId(group.to_string()));
        msg
    }

    fn make_msg_with_attachment(id: &str, conv: &str, text: &str) -> InboundMessage {
        let mut msg = make_msg(id, conv, text);
        msg.attachments.push(Attachment {
            id: format!("att-{id}"),
            mime_type: "image/png".to_string(),
            filename: Some(format!("{id}.png")),
            size: Some(1024),
            url: None,
            path: None,
            data: None,
        });
        msg
    }

    // -----------------------------------------------------------------------
    // Test: 3 messages same conversation -> 1 merged after tick
    // -----------------------------------------------------------------------
    #[test]
    fn merges_three_messages_same_conversation() {
        let cfg = CoalescingConfig {
            debounce_ms: 0, // expire immediately
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-a", "hello"));
        coalescer.push(make_msg("3", "chat-a", "world"));
        coalescer.push(make_msg("2", "chat-a", "there"));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1, "should merge into 1 message");
        let merged = &results[0];
        assert_eq!(merged.text, "hello\nworld\nthere");
        assert_eq!(
            merged.id.as_str(),
            "3",
            "should pick lexicographically max id"
        );
        assert_eq!(merged.conversation_id.as_str(), "chat-a");
    }

    // -----------------------------------------------------------------------
    // Test: early flush with '?' ending
    // -----------------------------------------------------------------------
    #[test]
    fn early_flush_on_question_mark() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000, // very long — would not expire normally
            early_flush_ms: 0,    // expire immediately on sentence end
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-b", "are you there?"));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "are you there?");
    }

    // -----------------------------------------------------------------------
    // Test: early flush with Chinese punctuation
    // -----------------------------------------------------------------------
    #[test]
    fn early_flush_on_chinese_punctuation() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000,
            early_flush_ms: 0,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-c", "你好。"));
        let results = coalescer.tick();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "你好。");
    }

    // -----------------------------------------------------------------------
    // Test: media_group_id aggregation
    // -----------------------------------------------------------------------
    #[test]
    fn media_group_aggregation() {
        let cfg = CoalescingConfig {
            media_group_timeout_ms: 0, // expire immediately
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_media_msg("1", "chat-d", "album-1", "photo 1"));
        coalescer.push(make_media_msg("2", "chat-d", "album-1", "photo 2"));
        coalescer.push(make_media_msg("3", "chat-d", "album-1", ""));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1);
        let merged = &results[0];
        assert_eq!(merged.text, "photo 1\nphoto 2");
        // All three metadata entries collected.
        assert_eq!(merged.metadata.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Test: max_fragments limit triggers immediate flush
    // -----------------------------------------------------------------------
    #[test]
    fn max_fragments_immediate_flush() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000,
            max_fragments: 3,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        // Push 2 — no flush yet.
        assert!(coalescer.push(make_msg("1", "chat-e", "a")).is_empty());
        assert!(coalescer.push(make_msg("2", "chat-e", "b")).is_empty());

        // 3rd triggers flush.
        let flushed = coalescer.push(make_msg("3", "chat-e", "c"));
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].text, "a\nb\nc");

        // Buffer is now empty — tick should return nothing.
        let tick_results = coalescer.tick();
        assert!(
            tick_results.is_empty(),
            "buffer should be empty after forced flush"
        );
    }

    // -----------------------------------------------------------------------
    // Test: max_bytes limit triggers immediate flush
    // -----------------------------------------------------------------------
    #[test]
    fn max_bytes_immediate_flush() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000,
            max_bytes: 10,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        assert!(coalescer.push(make_msg("1", "chat-f", "abcde")).is_empty()); // 5 bytes
        let flushed = coalescer.push(make_msg("2", "chat-f", "fghij")); // 10 total -> flush
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].text, "abcde\nfghij");
    }

    // -----------------------------------------------------------------------
    // Test: independent conversation buffers
    // -----------------------------------------------------------------------
    #[test]
    fn independent_conversation_buffers() {
        let cfg = CoalescingConfig {
            debounce_ms: 0, // expire immediately
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-x", "x1"));
        coalescer.push(make_msg("2", "chat-y", "y1"));
        coalescer.push(make_msg("3", "chat-x", "x2"));
        coalescer.push(make_msg("4", "chat-y", "y2"));

        let results = coalescer.tick();
        assert_eq!(results.len(), 2);

        // Sort for deterministic assertion.
        let mut sorted: Vec<_> = results.iter().map(|m| m.conversation_id.as_str()).collect();
        sorted.sort();
        assert_eq!(sorted, vec!["chat-x", "chat-y"]);

        for r in &results {
            match r.conversation_id.as_str() {
                "chat-x" => assert_eq!(r.text, "x1\nx2"),
                "chat-y" => assert_eq!(r.text, "y1\ny2"),
                other => panic!("unexpected conversation: {other}"),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test: flush_all empties everything
    // -----------------------------------------------------------------------
    #[test]
    fn flush_all_empties_everything() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000, // won't expire via tick
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-g", "hello"));
        coalescer.push(make_msg("2", "chat-h", "world"));

        // tick should return nothing (long deadline).
        assert!(coalescer.tick().is_empty());

        // flush_all should drain everything.
        let results = coalescer.flush_all();
        assert_eq!(results.len(), 2);

        // Subsequent flush_all should be empty.
        assert!(coalescer.flush_all().is_empty());
    }

    // -----------------------------------------------------------------------
    // Test: attachments are collected from all fragments
    // -----------------------------------------------------------------------
    #[test]
    fn attachments_collected_from_all_fragments() {
        let cfg = CoalescingConfig {
            debounce_ms: 0,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg_with_attachment("1", "chat-i", "pic 1"));
        coalescer.push(make_msg_with_attachment("2", "chat-i", "pic 2"));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].attachments.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test: single message returns as-is (no unnecessary clone)
    // -----------------------------------------------------------------------
    #[test]
    fn single_message_returns_unchanged() {
        let cfg = CoalescingConfig {
            debounce_ms: 0,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("42", "chat-j", "solo"));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.as_str(), "42");
        assert_eq!(results[0].text, "solo");
    }

    // -----------------------------------------------------------------------
    // Test: is_sentence_end helper
    // -----------------------------------------------------------------------
    #[test]
    fn sentence_end_detection() {
        assert!(is_sentence_end("hello?"));
        assert!(is_sentence_end("好的。"));
        assert!(is_sentence_end("wow！"));
        assert!(is_sentence_end("done."));
        assert!(is_sentence_end("真的吗？"));
        assert!(!is_sentence_end("hello"));
        assert!(!is_sentence_end("hello,"));
        assert!(!is_sentence_end(""));
        assert!(!is_sentence_end("   "));
    }
}

//! Message Coalescer
//!
//! Merges rapid-fire inbound messages from the same conversation (or media group)
//! into a single `InboundMessage` before handing off to the agent loop.
//!
//! # Design
//!
//! - **`DashMap`** for lock-free concurrent buffer access
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Hard ceiling on total coalescing latency, measured from the first
    /// buffered fragment (ms). The rolling `debounce_ms` window resets on every
    /// new message; without this ceiling a slow drip of fragments (one every
    /// `debounce_ms - ε`) would delay dispatch until a `max_fragments` /
    /// `max_bytes` safety limit is hit. `0` disables the ceiling (rolling
    /// window only — legacy behavior).
    #[serde(default = "default_max_window_ms")]
    pub max_window_ms: u64,
}

/// Default hard-deadline ceiling for coalescing (ms). Kept as a free function so
/// `#[serde(default)]` can backfill it when deserializing older configs.
const fn default_max_window_ms() -> u64 {
    5_000
}

impl Default for CoalescingConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 800,
            early_flush_ms: 200,
            media_group_timeout_ms: 500,
            max_fragments: 12,
            max_bytes: 50_000,
            max_window_ms: default_max_window_ms(),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal buffer
// ---------------------------------------------------------------------------

struct CoalesceBuffer {
    messages: Vec<InboundMessage>,
    deadline: Instant,
    /// Timestamp of the first fragment in the current batch. Anchors the hard
    /// `max_window_ms` ceiling so the rolling deadline can never escape it.
    first_seen: Instant,
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
    #[must_use]
    pub fn new(config: CoalescingConfig) -> Self {
        Self {
            buffers: DashMap::new(),
            config,
        }
    }

    /// Ingest a message. Returns any immediately-flushed results when a buffer
    /// exceeds safety limits (`max_fragments` / `max_bytes`).
    pub fn push(&self, msg: InboundMessage) -> Vec<InboundMessage> {
        let now = Instant::now();
        let conversation_id = msg.conversation_id.as_str().to_owned();

        // Choose the buffer key: media_group_id takes precedence.
        //
        // Scope the key by SENDER as well as conversation. In a group chat
        // several people post into one conversation; keying only on the
        // conversation would coalesce their messages into a single buffer, and
        // `merge()` keeps only the first fragment's `sender_id`/`sender_name` —
        // so everyone's text would be attributed to whoever happened to send
        // first. Folding `sender_id` into the key isolates each sender, so a
        // merged message only ever carries one person's fragments and its
        // attribution stays correct. In a 1:1 chat there is a single human
        // sender, so this is a no-op there.
        let is_media_group = msg.meta_media_group_id().is_some();
        let sender = msg.sender_id.as_str();
        let key = if let Some(mg) = msg.meta_media_group_id() {
            // Scope the media-group key to the conversation: Telegram's
            // media_group_id is not globally unique across chats, so an
            // unscoped key can merge fragments from different conversations. A
            // media group is inherently single-sender (one user's album), but
            // fold `sender` in too for symmetry and defence against adapters
            // that reuse a media_group_id across senders.
            format!("mg:{conversation_id}:{sender}:{mg}")
        } else {
            format!("cv:{conversation_id}:{sender}")
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
            first_seen: now,
            total_text_bytes: 0,
            media_group_id: media_group_id.clone(),
        });

        let buf = entry.value_mut();
        buf.messages.push(msg);
        buf.total_text_bytes += text_len;
        // Roll the debounce window forward, but never past the hard ceiling
        // anchored at the first fragment. `max_window_ms == 0` disables the
        // ceiling (legacy rolling-window-only behavior).
        buf.deadline = if self.config.max_window_ms > 0 {
            let hard = buf.first_seen + Duration::from_millis(self.config.max_window_ms);
            base_deadline.min(hard)
        } else {
            base_deadline
        };

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
            // A forced flush closes the batch; re-anchor the hard window so any
            // subsequent fragments start a fresh ceiling rather than inheriting
            // the (now-elapsed) original anchor.
            buf.first_seen = now;
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
                        .map_or("unknown", |m| m.conversation_id.as_str());
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

            // Compare numerically when both ids are integers (e.g. Telegram
            // message ids are unpadded decimals, where lexicographic "99" > "100"
            // is wrong); fall back to lexicographic for non-numeric channel ids.
            let is_newer = match (msg.id.as_str().parse::<u64>(), max_id.parse::<u64>()) {
                (Ok(a), Ok(b)) => a > b,
                _ => msg.id.as_str() > max_id.as_str(),
            };
            if is_newer {
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
    // Test (#8): distinct senders in one group conversation must NOT merge —
    // otherwise `merge()` keeps only the first fragment's sender and everyone's
    // text is misattributed to whoever posted first.
    // -----------------------------------------------------------------------
    #[test]
    fn distinct_senders_in_a_group_do_not_merge() {
        let cfg = CoalescingConfig {
            debounce_ms: 0, // expire immediately
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        let mut alice = make_msg("1", "group-x", "from alice");
        alice.sender_id = UserId::new("alice");
        alice.is_group = true;
        let mut bob = make_msg("2", "group-x", "from bob");
        bob.sender_id = UserId::new("bob");
        bob.is_group = true;

        coalescer.push(alice);
        coalescer.push(bob);

        let mut results = coalescer.tick();
        assert_eq!(
            results.len(),
            2,
            "two senders in one conversation must not coalesce into one message"
        );
        // Each single-fragment message keeps its own sender attribution.
        results.sort_by(|a, b| a.sender_id.as_str().cmp(b.sender_id.as_str()));
        assert_eq!(results[0].sender_id.as_str(), "alice");
        assert_eq!(results[0].text, "from alice");
        assert_eq!(results[1].sender_id.as_str(), "bob");
        assert_eq!(results[1].text, "from bob");
    }

    /// A single sender's consecutive messages in a group still coalesce — the
    /// per-sender key only isolates *different* people.
    #[test]
    fn same_sender_in_a_group_still_merges() {
        let cfg = CoalescingConfig {
            debounce_ms: 0,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        let mut m1 = make_msg("1", "group-x", "part one");
        m1.sender_id = UserId::new("alice");
        m1.is_group = true;
        let mut m2 = make_msg("2", "group-x", "part two");
        m2.sender_id = UserId::new("alice");
        m2.is_group = true;

        coalescer.push(m1);
        coalescer.push(m2);

        let results = coalescer.tick();
        assert_eq!(results.len(), 1, "one sender's burst still coalesces");
        assert_eq!(results[0].text, "part one\npart two");
        assert_eq!(results[0].sender_id.as_str(), "alice");
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
    // Test: hard max_window ceiling forces flush despite a long rolling window
    // -----------------------------------------------------------------------
    #[test]
    fn hard_window_caps_rolling_debounce() {
        // Rolling window is effectively infinite (debounce + early_flush huge),
        // so without the ceiling these fragments would never expire via tick.
        // The 10ms hard ceiling, anchored at the first fragment, forces a flush.
        let cfg = CoalescingConfig {
            debounce_ms: 100_000,
            early_flush_ms: 100_000,
            max_window_ms: 10,
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        // Two non-sentence fragments; the second resets the rolling window but
        // must NOT escape the hard ceiling anchored at the first.
        coalescer.push(make_msg("1", "chat-w", "drip one"));
        coalescer.push(make_msg("2", "chat-w", "drip two"));

        // Before the ceiling elapses, nothing should flush.
        assert!(
            coalescer.tick().is_empty(),
            "should not flush before the hard window elapses"
        );

        // Sleep well past the 10ms ceiling (5x margin keeps this non-flaky).
        std::thread::sleep(std::time::Duration::from_millis(50));

        let results = coalescer.tick();
        assert_eq!(results.len(), 1, "hard window must force a flush");
        assert_eq!(results[0].text, "drip one\ndrip two");
    }

    // -----------------------------------------------------------------------
    // Test: max_window_ms == 0 disables the ceiling (legacy behavior)
    // -----------------------------------------------------------------------
    #[test]
    fn hard_window_disabled_keeps_rolling() {
        let cfg = CoalescingConfig {
            debounce_ms: 100_000,
            early_flush_ms: 100_000,
            max_window_ms: 0, // disabled
            ..Default::default()
        };
        let coalescer = MessageCoalescer::new(cfg);

        coalescer.push(make_msg("1", "chat-z", "stay"));
        std::thread::sleep(std::time::Duration::from_millis(20));

        // With the ceiling disabled and a huge rolling window, tick must NOT flush.
        assert!(
            coalescer.tick().is_empty(),
            "disabled ceiling should leave the long rolling window intact"
        );
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

//! Debounce buffer for merging rapid-fire messages.
//!
//! Collects messages per session and merges them after a configurable
//! sliding window expires. Supports immediate flush on max-messages
//! or hard deadline.

use std::collections::HashMap;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

use crate::gateway::inbound_context::InboundContext;
use crate::sync_primitives::Arc;

use super::types::MergedMessage;

// ---------------------------------------------------------------------------
// DebounceConfig
// ---------------------------------------------------------------------------

/// Configuration for debounce behavior.
#[derive(Debug, Clone)]
pub struct DebounceConfig {
    /// Default sliding window in milliseconds.
    pub default_window_ms: u64,
    /// Hard deadline since the first message in a batch.
    pub max_window_ms: u64,
    /// Flush immediately when this many messages accumulate.
    pub max_messages: usize,
    /// Per-channel window overrides (channel_id -> window_ms).
    pub channel_overrides: HashMap<String, u64>,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            default_window_ms: 2000,
            max_window_ms: 5000,
            max_messages: 10,
            channel_overrides: HashMap::new(),
        }
    }
}

impl DebounceConfig {
    /// Get the debounce window for a specific channel, falling back to default.
    pub fn window_for_channel(&self, channel_id: &str) -> Duration {
        let ms = self
            .channel_overrides
            .get(channel_id)
            .copied()
            .unwrap_or(self.default_window_ms);
        Duration::from_millis(ms)
    }

    /// Get the hard deadline duration.
    pub fn max_window(&self) -> Duration {
        Duration::from_millis(self.max_window_ms)
    }
}

// ---------------------------------------------------------------------------
// Callback type
// ---------------------------------------------------------------------------

/// Callback invoked when a debounced batch is ready.
pub type OnMergedReady = Arc<dyn Fn(MergedMessage) + Send + Sync>;

// ---------------------------------------------------------------------------
// DebounceBatch (private)
// ---------------------------------------------------------------------------

struct DebounceBatch {
    contexts: Vec<InboundContext>,
    first_received: Instant,
    timer_handle: Option<JoinHandle<()>>,
}

// ---------------------------------------------------------------------------
// DebounceBuffer
// ---------------------------------------------------------------------------

/// Collects rapid-fire messages per session and merges them after a
/// configurable sliding window expires.
pub struct DebounceBuffer {
    pending: Arc<Mutex<HashMap<String, DebounceBatch>>>,
    config: DebounceConfig,
    on_ready: OnMergedReady,
}

impl DebounceBuffer {
    /// Create a new debounce buffer.
    pub fn new(config: DebounceConfig, on_ready: OnMergedReady) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            config,
            on_ready,
        }
    }

    /// Submit a message for debouncing.
    ///
    /// Groups messages by session key. Flushes immediately if max_messages
    /// or max_window is reached; otherwise resets the sliding timer.
    pub async fn submit(&self, ctx: InboundContext) {
        let key = ctx.session_key.to_key_string();
        let channel_id = ctx.message.channel_id.as_str().to_string();
        let mut pending = self.pending.lock().await;

        let batch = pending.entry(key.clone()).or_insert_with(|| DebounceBatch {
            contexts: Vec::new(),
            first_received: Instant::now(),
            timer_handle: None,
        });

        batch.contexts.push(ctx);

        // Check immediate flush: max messages reached
        if batch.contexts.len() >= self.config.max_messages {
            let contexts = Self::take_batch(&mut pending, &key);
            let on_ready = Arc::clone(&self.on_ready);
            // Drop lock before calling callback
            drop(pending);
            on_ready(MergedMessage::from_batch(contexts));
            return;
        }

        // Check immediate flush: hard deadline exceeded
        let elapsed = batch.first_received.elapsed();
        if elapsed >= self.config.max_window() {
            let contexts = Self::take_batch(&mut pending, &key);
            let on_ready = Arc::clone(&self.on_ready);
            drop(pending);
            on_ready(MergedMessage::from_batch(contexts));
            return;
        }

        // Cancel previous timer
        if let Some(handle) = batch.timer_handle.take() {
            handle.abort();
        }

        // Calculate sliding window, clamped to remaining max_window
        let window = self.config.window_for_channel(&channel_id);
        let remaining = self.config.max_window().saturating_sub(elapsed);
        let delay = window.min(remaining);

        // Spawn new timer
        let pending_clone = Arc::clone(&self.pending);
        let on_ready = Arc::clone(&self.on_ready);
        let key_clone = key.clone();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut pending = pending_clone.lock().await;
            if let Some(batch) = pending.remove(&key_clone) {
                if !batch.contexts.is_empty() {
                    let on_ready = on_ready;
                    let contexts = batch.contexts;
                    drop(pending);
                    on_ready(MergedMessage::from_batch(contexts));
                }
            }
        });

        // Store the handle — we need to re-acquire the entry since we may
        // have dropped and re-acquired nothing yet (lock is still held here)
        if let Some(batch) = pending.get_mut(&key) {
            batch.timer_handle = Some(handle);
        }
    }

    /// Number of sessions with pending (not yet flushed) batches.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Extract and remove a batch from the pending map, aborting its timer.
    fn take_batch(
        pending: &mut HashMap<String, DebounceBatch>,
        key: &str,
    ) -> Vec<InboundContext> {
        if let Some(mut batch) = pending.remove(key) {
            if let Some(handle) = batch.timer_handle.take() {
                handle.abort();
            }
            batch.contexts
        } else {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tokio::sync::Notify;

    use crate::gateway::channel::{
        ChannelId, ConversationId, InboundMessage, MessageId, UserId,
    };
    use crate::gateway::inbound_context::ReplyRoute;
    use crate::gateway::router::SessionKey;

    fn make_context(text: &str, msg_id: &str) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new(msg_id),
            channel_id: ChannelId::new("test-ch"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test-ch"), ConversationId::new("conv-1"));
        let session_key = SessionKey::main("main");
        InboundContext::new(msg, route, session_key)
    }

    fn make_context_with_session(
        text: &str,
        msg_id: &str,
        session_key: SessionKey,
    ) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new(msg_id),
            channel_id: ChannelId::new("test-ch"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test-ch"), ConversationId::new("conv-1"));
        InboundContext::new(msg, route, session_key)
    }

    fn make_context_with_channel(
        text: &str,
        msg_id: &str,
        channel_id: &str,
    ) -> InboundContext {
        let msg = InboundMessage {
            id: MessageId::new(msg_id),
            channel_id: ChannelId::new(channel_id),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(
            ChannelId::new(channel_id),
            ConversationId::new("conv-1"),
        );
        let session_key = SessionKey::main("main");
        InboundContext::new(msg, route, session_key)
    }

    #[tokio::test]
    async fn test_single_message_passthrough() {
        let notify = Arc::new(Notify::new());
        let result: Arc<Mutex<Option<MergedMessage>>> = Arc::new(Mutex::new(None));

        let notify_cb = Arc::clone(&notify);
        let result_cb = Arc::clone(&result);

        let config = DebounceConfig {
            default_window_ms: 100,
            max_window_ms: 500,
            max_messages: 10,
            ..Default::default()
        };

        let buffer = DebounceBuffer::new(
            config,
            Arc::new(move |merged| {
                let result_cb = Arc::clone(&result_cb);
                let notify_cb = Arc::clone(&notify_cb);
                // Use try_lock since we're in a sync callback
                tokio::spawn(async move {
                    *result_cb.lock().await = Some(merged);
                    notify_cb.notify_one();
                });
            }),
        );

        buffer.submit(make_context("hello", "m1")).await;
        assert_eq!(buffer.pending_count().await, 1);

        notify.notified().await;

        let guard = result.lock().await;
        let merged = guard.as_ref().expect("callback should have fired");
        assert_eq!(merged.merge_count, 1);
        assert_eq!(merged.text, "hello");
    }

    #[tokio::test]
    async fn test_rapid_fire_merge() {
        let notify = Arc::new(Notify::new());
        let result: Arc<Mutex<Option<MergedMessage>>> = Arc::new(Mutex::new(None));

        let notify_cb = Arc::clone(&notify);
        let result_cb = Arc::clone(&result);

        let config = DebounceConfig {
            default_window_ms: 200,
            max_window_ms: 5000,
            max_messages: 10,
            ..Default::default()
        };

        let buffer = DebounceBuffer::new(
            config,
            Arc::new(move |merged| {
                let result_cb = Arc::clone(&result_cb);
                let notify_cb = Arc::clone(&notify_cb);
                tokio::spawn(async move {
                    *result_cb.lock().await = Some(merged);
                    notify_cb.notify_one();
                });
            }),
        );

        // Send 3 messages rapidly (within the 200ms window)
        buffer.submit(make_context("one", "m1")).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        buffer.submit(make_context("two", "m2")).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        buffer.submit(make_context("three", "m3")).await;

        // Wait for the window to expire
        notify.notified().await;

        let guard = result.lock().await;
        let merged = guard.as_ref().expect("callback should have fired");
        assert_eq!(merged.merge_count, 3);
        assert_eq!(merged.text, "one\ntwo\nthree");
    }

    #[tokio::test]
    async fn test_max_messages_flush() {
        let notify = Arc::new(Notify::new());
        let result: Arc<Mutex<Option<MergedMessage>>> = Arc::new(Mutex::new(None));

        let notify_cb = Arc::clone(&notify);
        let result_cb = Arc::clone(&result);

        let config = DebounceConfig {
            default_window_ms: 5000, // long window — should not fire
            max_window_ms: 10000,
            max_messages: 3,
            ..Default::default()
        };

        let buffer = DebounceBuffer::new(
            config,
            Arc::new(move |merged| {
                let result_cb = Arc::clone(&result_cb);
                let notify_cb = Arc::clone(&notify_cb);
                tokio::spawn(async move {
                    *result_cb.lock().await = Some(merged);
                    notify_cb.notify_one();
                });
            }),
        );

        buffer.submit(make_context("a", "m1")).await;
        buffer.submit(make_context("b", "m2")).await;

        // Not flushed yet
        assert_eq!(buffer.pending_count().await, 1);

        // Third message triggers immediate flush
        buffer.submit(make_context("c", "m3")).await;

        // Should have flushed synchronously (no need to wait for timer)
        // Give a tiny moment for the spawn in callback
        tokio::time::sleep(Duration::from_millis(10)).await;
        notify.notified().await;

        let guard = result.lock().await;
        let merged = guard.as_ref().expect("callback should have fired");
        assert_eq!(merged.merge_count, 3);
        assert_eq!(merged.text, "a\nb\nc");
        drop(guard);

        // Pending should be empty now
        assert_eq!(buffer.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_separate_sessions_independent() {
        let results: Arc<Mutex<Vec<MergedMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());

        let results_cb = Arc::clone(&results);
        let notify_cb = Arc::clone(&notify);

        let config = DebounceConfig {
            default_window_ms: 100,
            max_window_ms: 5000,
            max_messages: 10,
            ..Default::default()
        };

        let buffer = DebounceBuffer::new(
            config,
            Arc::new(move |merged| {
                let results_cb = Arc::clone(&results_cb);
                let notify_cb = Arc::clone(&notify_cb);
                tokio::spawn(async move {
                    results_cb.lock().await.push(merged);
                    notify_cb.notify_one();
                });
            }),
        );

        // Submit to two different sessions
        let ctx1 = make_context_with_session("hello", "m1", SessionKey::main("agent-a"));
        let ctx2 = make_context_with_session("world", "m2", SessionKey::main("agent-b"));

        buffer.submit(ctx1).await;
        buffer.submit(ctx2).await;

        assert_eq!(buffer.pending_count().await, 2);

        // Wait for both to flush
        notify.notified().await;
        notify.notified().await;

        let guard = results.lock().await;
        assert_eq!(guard.len(), 2);

        // Each should have merge_count=1 (independent sessions)
        assert!(guard.iter().all(|m| m.merge_count == 1));
        let texts: Vec<&str> = guard.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"hello"));
        assert!(texts.contains(&"world"));
    }

    #[tokio::test]
    async fn test_config_channel_override() {
        let notify = Arc::new(Notify::new());
        let result: Arc<Mutex<Option<MergedMessage>>> = Arc::new(Mutex::new(None));

        let notify_cb = Arc::clone(&notify);
        let result_cb = Arc::clone(&result);

        let mut overrides = HashMap::new();
        overrides.insert("fast-channel".to_string(), 50u64); // 50ms override

        let config = DebounceConfig {
            default_window_ms: 5000, // very long default
            max_window_ms: 10000,
            max_messages: 100,
            channel_overrides: overrides,
        };

        // Verify the config method works
        assert_eq!(
            config.window_for_channel("fast-channel"),
            Duration::from_millis(50)
        );
        assert_eq!(
            config.window_for_channel("other"),
            Duration::from_millis(5000)
        );

        let buffer = DebounceBuffer::new(
            config,
            Arc::new(move |merged| {
                let result_cb = Arc::clone(&result_cb);
                let notify_cb = Arc::clone(&notify_cb);
                tokio::spawn(async move {
                    *result_cb.lock().await = Some(merged);
                    notify_cb.notify_one();
                });
            }),
        );

        // Submit to the fast channel — should flush after ~50ms, not 5000ms
        let ctx = make_context_with_channel("fast msg", "m1", "fast-channel");
        buffer.submit(ctx).await;

        // Should fire within 200ms (50ms window + tolerance)
        tokio::time::sleep(Duration::from_millis(200)).await;
        notify.notified().await;

        let guard = result.lock().await;
        let merged = guard.as_ref().expect("callback should have fired quickly");
        assert_eq!(merged.merge_count, 1);
        assert_eq!(merged.text, "fast msg");
    }
}

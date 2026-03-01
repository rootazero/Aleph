//! Stream subscriber with callbacks.
//!
//! Provides callback-based subscription to streaming events.

use crate::sync_primitives::Arc;
use tokio::sync::mpsc;

use super::events::StreamEvent;

/// Callback type for stream events
pub type StreamCallback = Arc<dyn Fn(StreamEvent) + Send + Sync>;

/// Stream subscriber for handling streaming events
#[derive(Clone)]
pub struct StreamSubscriber {
    callbacks: Vec<StreamCallback>,
    sender: Option<mpsc::Sender<StreamEvent>>,
}

impl Default for StreamSubscriber {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamSubscriber {
    /// Create a new stream subscriber
    pub fn new() -> Self {
        Self {
            callbacks: Vec::new(),
            sender: None,
        }
    }

    /// Create with a channel sender
    pub fn with_channel(sender: mpsc::Sender<StreamEvent>) -> Self {
        Self {
            callbacks: Vec::new(),
            sender: Some(sender),
        }
    }

    /// Add a callback to be invoked on each event
    pub fn on_event<F>(&mut self, callback: F)
    where
        F: Fn(StreamEvent) + Send + Sync + 'static,
    {
        self.callbacks.push(Arc::new(callback));
    }

    /// Emit an event to all subscribers
    pub async fn emit(&self, event: StreamEvent) {
        // Invoke callbacks
        for callback in &self.callbacks {
            callback(event.clone());
        }

        // Send to channel if configured
        if let Some(sender) = &self.sender {
            let _ = sender.send(event).await;
        }
    }

    /// Emit an event synchronously (blocking)
    pub fn emit_sync(&self, event: StreamEvent) {
        // Invoke callbacks
        for callback in &self.callbacks {
            callback(event.clone());
        }

        // Send to channel if configured (non-blocking try_send)
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(event);
        }
    }

    /// Emit a text delta event
    pub async fn emit_text_delta(&self, delta: &str, accumulated: &str) {
        self.emit(StreamEvent::TextDelta {
            delta: delta.to_string(),
            accumulated: accumulated.to_string(),
        })
        .await;
    }

    /// Emit a thinking delta event
    pub async fn emit_thinking_delta(&self, delta: &str, accumulated: &str) {
        self.emit(StreamEvent::ThinkingDelta {
            delta: delta.to_string(),
            accumulated: accumulated.to_string(),
        })
        .await;
    }

    /// Emit thinking complete event
    pub async fn emit_thinking_complete(&self, content: &str) {
        self.emit(StreamEvent::ThinkingComplete {
            content: content.to_string(),
        })
        .await;
    }

    /// Emit block reply event
    pub async fn emit_block_reply(&self, text: &str, is_final: bool) {
        self.emit(StreamEvent::BlockReply {
            text: text.to_string(),
            is_final,
        })
        .await;
    }

    /// Emit tool start event
    pub async fn emit_tool_start(&self, tool_id: &str, tool_name: &str) {
        self.emit(StreamEvent::ToolStart {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
        })
        .await;
    }

    /// Emit tool complete event
    pub async fn emit_tool_complete(&self, tool_id: &str, result: serde_json::Value) {
        self.emit(StreamEvent::ToolComplete {
            tool_id: tool_id.to_string(),
            result,
        })
        .await;
    }

    /// Emit error event
    pub async fn emit_error(&self, message: &str, recoverable: bool) {
        self.emit(StreamEvent::Error {
            message: message.to_string(),
            recoverable,
        })
        .await;
    }

    /// Check if any subscribers are registered
    pub fn has_subscribers(&self) -> bool {
        !self.callbacks.is_empty() || self.sender.is_some()
    }
}

/// Builder for StreamSubscriber
pub struct StreamSubscriberBuilder {
    subscriber: StreamSubscriber,
}

impl StreamSubscriberBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            subscriber: StreamSubscriber::new(),
        }
    }

    /// Add callback for text deltas
    pub fn on_text<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.subscriber.on_event(move |event| {
            if let StreamEvent::TextDelta { delta, accumulated } = event {
                callback(&delta, &accumulated);
            }
        });
        self
    }

    /// Add callback for thinking deltas
    pub fn on_thinking<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.subscriber.on_event(move |event| {
            if let StreamEvent::ThinkingDelta { delta, accumulated } = event {
                callback(&delta, &accumulated);
            }
        });
        self
    }

    /// Add callback for block replies
    pub fn on_block_reply<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, bool) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.subscriber.on_event(move |event| {
            if let StreamEvent::BlockReply { text, is_final } = event {
                callback(&text, is_final);
            }
        });
        self
    }

    /// Add callback for errors
    pub fn on_error<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, bool) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.subscriber.on_event(move |event| {
            if let StreamEvent::Error { message, recoverable } = event {
                callback(&message, recoverable);
            }
        });
        self
    }

    /// Build the subscriber
    pub fn build(self) -> StreamSubscriber {
        self.subscriber
    }
}

impl Default for StreamSubscriberBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_callback_invocation() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let mut subscriber = StreamSubscriber::new();
        subscriber.on_event(move |_| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        subscriber.emit(StreamEvent::TextDelta {
            delta: "test".to_string(),
            accumulated: "test".to_string(),
        }).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_channel_subscriber() {
        let (tx, mut rx) = mpsc::channel(10);
        let subscriber = StreamSubscriber::with_channel(tx);

        subscriber.emit(StreamEvent::TextDelta {
            delta: "hello".to_string(),
            accumulated: "hello".to_string(),
        }).await;

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, StreamEvent::TextDelta { .. }));
    }

    #[test]
    fn test_builder() {
        let text_received = Arc::new(std::sync::Mutex::new(false));
        let text_clone = text_received.clone();

        let subscriber = StreamSubscriberBuilder::new()
            .on_text(move |_, _| {
                *text_clone.lock().unwrap() = true;
            })
            .build();

        subscriber.emit_sync(StreamEvent::TextDelta {
            delta: "test".to_string(),
            accumulated: "test".to_string(),
        });

        assert!(*text_received.lock().unwrap());
    }
}

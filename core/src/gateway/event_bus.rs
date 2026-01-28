//! Gateway Event Bus
//!
//! Provides a broadcast channel for pushing events to all connected WebSocket clients.
//! Events are JSON-RPC 2.0 notifications (requests without an id).

use tokio::sync::broadcast;
use tracing::debug;

/// Default channel capacity for event broadcasting
const EVENT_CHANNEL_SIZE: usize = 1024;

/// Event bus for broadcasting events to all connected clients
///
/// The event bus uses a broadcast channel to efficiently distribute
/// events to multiple subscribers. If a subscriber falls behind,
/// it will miss events rather than blocking the sender.
pub struct GatewayEventBus {
    sender: broadcast::Sender<String>,
}

impl GatewayEventBus {
    /// Create a new event bus with default channel size
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        Self { sender }
    }

    /// Create a new event bus with custom channel size
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publish an event to all subscribers
    ///
    /// The event should be a JSON-encoded string. Events are delivered
    /// asynchronously; this method returns immediately.
    ///
    /// # Arguments
    ///
    /// * `event` - JSON-encoded event string
    ///
    /// # Returns
    ///
    /// Number of receivers that will receive the event
    pub fn publish(&self, event: String) -> usize {
        let preview = if event.len() > 100 {
            format!("{}...", &event[..100])
        } else {
            event.clone()
        };
        debug!("Publishing event: {}", preview);

        // send returns Err if there are no receivers, which is fine
        self.sender.send(event).unwrap_or(0)
    }

    /// Publish a typed event by serializing it to JSON
    ///
    /// # Arguments
    ///
    /// * `event` - Event object that implements Serialize
    ///
    /// # Returns
    ///
    /// Number of receivers, or error if serialization fails
    pub fn publish_json<T: serde::Serialize>(&self, event: &T) -> Result<usize, serde_json::Error> {
        let json = serde_json::to_string(event)?;
        Ok(self.publish(json))
    }

    /// Subscribe to receive events
    ///
    /// Returns a receiver that will receive all events published after
    /// this call. The receiver should be polled in a loop.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }

    /// Get the current number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for GatewayEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for GatewayEventBus {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_publish_subscribe() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(r#"{"event":"test"}"#.to_string());

        let received = rx.recv().await.unwrap();
        assert!(received.contains("test"));
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = GatewayEventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let count = bus.publish(r#"{"event":"multi"}"#.to_string());
        assert_eq!(count, 2);

        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[test]
    fn test_no_subscribers() {
        let bus = GatewayEventBus::new();
        // Should not panic when there are no subscribers
        let count = bus.publish("test".to_string());
        assert_eq!(count, 0);
    }

    #[test]
    fn test_subscriber_count() {
        let bus = GatewayEventBus::new();
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }
}

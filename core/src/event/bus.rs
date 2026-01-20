// Aether/core/src/event/bus.rs
//! Event bus implementation using tokio broadcast channels.

use crate::event::types::{AetherEvent, EventType, TimestampedEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, trace, warn};

/// Default buffer size for the broadcast channel
const DEFAULT_BUFFER_SIZE: usize = 1024;

/// Maximum history size to keep
const MAX_HISTORY_SIZE: usize = 10000;

/// Event bus for component communication
///
/// Uses tokio broadcast channel for multi-subscriber support.
/// Events are type-safe and all subscribers receive all events.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<TimestampedEvent>,
    history: Arc<RwLock<Vec<TimestampedEvent>>>,
    config: EventBusConfig,
}

/// Configuration for the event bus
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Buffer size for the broadcast channel
    pub buffer_size: usize,
    /// Whether to keep event history
    pub enable_history: bool,
    /// Maximum history entries to keep
    pub max_history_size: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
            enable_history: true,
            max_history_size: MAX_HISTORY_SIZE,
        }
    }
}

/// Subscriber handle for receiving events
pub struct EventSubscriber {
    receiver: broadcast::Receiver<TimestampedEvent>,
    filter: Vec<EventType>,
}

impl EventBus {
    /// Create a new event bus with default configuration
    pub fn new() -> Self {
        Self::with_config(EventBusConfig::default())
    }

    /// Create a new event bus with custom buffer size
    pub fn with_buffer_size(buffer_size: usize) -> Self {
        Self::with_config(EventBusConfig {
            buffer_size,
            ..Default::default()
        })
    }

    /// Create a new event bus with custom configuration
    pub fn with_config(config: EventBusConfig) -> Self {
        let (sender, _) = broadcast::channel(config.buffer_size);
        Self {
            sender,
            history: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    /// Publish an event to all subscribers
    ///
    /// Returns the number of active subscribers that received the event.
    pub async fn publish(&self, event: AetherEvent) -> usize {
        let timestamped = TimestampedEvent::new(event);

        trace!(
            event_type = ?timestamped.event.event_type(),
            sequence = timestamped.sequence,
            "Publishing event"
        );

        // Store in history if enabled
        if self.config.enable_history {
            let mut history = self.history.write().await;
            history.push(timestamped.clone());

            // Trim history if too large
            if history.len() > self.config.max_history_size {
                let drain_count = history.len() - self.config.max_history_size;
                history.drain(0..drain_count);
                debug!(drain_count, "Trimmed event history");
            }
        }

        // Send to subscribers
        match self.sender.send(timestamped) {
            Ok(count) => {
                trace!(subscriber_count = count, "Event delivered");
                count
            }
            Err(_) => {
                // No subscribers - this is not an error
                trace!("No subscribers for event");
                0
            }
        }
    }

    /// Subscribe to all events
    pub fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
            filter: vec![EventType::All],
        }
    }

    /// Subscribe to specific event types
    pub fn subscribe_filtered(&self, event_types: Vec<EventType>) -> EventSubscriber {
        EventSubscriber {
            receiver: self.sender.subscribe(),
            filter: event_types,
        }
    }

    /// Get the current number of subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Get event history
    pub async fn history(&self) -> Vec<TimestampedEvent> {
        self.history.read().await.clone()
    }

    /// Get history since a specific sequence number
    pub async fn history_since(&self, since_sequence: u64) -> Vec<TimestampedEvent> {
        self.history
            .read()
            .await
            .iter()
            .filter(|e| e.sequence > since_sequence)
            .cloned()
            .collect()
    }

    /// Clear event history
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSubscriber {
    /// Receive the next event
    ///
    /// Blocks until an event is available or the channel is closed.
    /// If filtering is enabled, only matching events are returned.
    pub async fn recv(&mut self) -> Result<TimestampedEvent, EventBusError> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if self.matches(&event) {
                        return Ok(event);
                    }
                    // Event doesn't match filter, continue waiting
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    warn!(
                        lagged_count = count,
                        "Subscriber lagged behind, some events were missed"
                    );
                    // Continue receiving
                }
            }
        }
    }

    /// Try to receive an event without blocking
    pub fn try_recv(&mut self) -> Result<Option<TimestampedEvent>, EventBusError> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    if self.matches(&event) {
                        return Ok(Some(event));
                    }
                    // Event doesn't match filter, try next
                }
                Err(broadcast::error::TryRecvError::Empty) => {
                    return Ok(None);
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(EventBusError::ChannelClosed);
                }
                Err(broadcast::error::TryRecvError::Lagged(count)) => {
                    warn!(
                        lagged_count = count,
                        "Subscriber lagged behind, some events were missed"
                    );
                    // Continue receiving
                }
            }
        }
    }

    /// Check if event matches the filter
    fn matches(&self, event: &TimestampedEvent) -> bool {
        if self.filter.contains(&EventType::All) {
            return true;
        }
        self.filter.contains(&event.event.event_type())
    }
}

/// Error type for event bus operations
#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    #[error("Event channel is closed")]
    ChannelClosed,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe();

        let event = AetherEvent::InputReceived(InputEvent {
            text: "hello".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        // Publish in a separate task
        let bus_clone = bus.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            bus_clone.publish(event).await;
        });

        // Receive
        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.event.event_type(), EventType::InputReceived);
    }

    #[tokio::test]
    async fn test_filtered_subscription() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe_filtered(vec![EventType::LoopStop]);

        // Publish non-matching event first
        bus.publish(AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        }))
        .await;

        // Publish matching event
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Should only receive LoopStop
        let received = subscriber.try_recv().unwrap();
        assert!(received.is_some());
        assert_eq!(received.unwrap().event.event_type(), EventType::LoopStop);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut sub1 = bus.subscribe();
        let mut sub2 = bus.subscribe();

        assert_eq!(bus.subscriber_count(), 2);

        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Both should receive
        let r1 = sub1.try_recv().unwrap();
        let r2 = sub2.try_recv().unwrap();

        assert!(r1.is_some());
        assert!(r2.is_some());
    }

    #[tokio::test]
    async fn test_event_history() {
        let bus = EventBus::new();

        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;
        bus.publish(AetherEvent::LoopStop(StopReason::UserAborted))
            .await;

        let history = bus.history().await;
        assert_eq!(history.len(), 2);

        let since = bus.history_since(history[0].sequence).await;
        assert_eq!(since.len(), 1);
    }

    #[tokio::test]
    async fn test_history_trimming() {
        let bus = EventBus::with_config(EventBusConfig {
            buffer_size: 16,
            enable_history: true,
            max_history_size: 5,
        });

        for _ in 0..10 {
            bus.publish(AetherEvent::LoopStop(StopReason::Completed))
                .await;
        }

        let history = bus.history().await;
        assert_eq!(history.len(), 5);
    }
}

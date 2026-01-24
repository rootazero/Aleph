// Aether/core/src/event/bus.rs
//! Event bus implementation using tokio broadcast channels.

use crate::event::global_bus::GlobalBus;
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
///
/// The EventBus can optionally be connected to a GlobalBus for cross-agent
/// event aggregation. When connected, all published events are automatically
/// broadcast to the GlobalBus with source context.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<TimestampedEvent>,
    history: Arc<RwLock<Vec<TimestampedEvent>>>,
    config: EventBusConfig,
    /// Optional agent ID for GlobalBus integration
    agent_id: Option<String>,
    /// Optional session ID for GlobalBus integration
    session_id: Option<String>,
    /// Reference to the GlobalBus for auto-broadcast
    global_bus: Option<&'static GlobalBus>,
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
            agent_id: None,
            session_id: None,
            global_bus: None,
        }
    }

    /// Set the agent ID for GlobalBus integration.
    ///
    /// When an agent_id is set and the EventBus is connected to a GlobalBus,
    /// all published events will be broadcast to the GlobalBus with this
    /// agent ID as the source.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The agent identifier
    pub fn with_agent_id(mut self, agent_id: &str) -> Self {
        self.agent_id = Some(agent_id.to_string());
        self
    }

    /// Set the session ID for GlobalBus integration.
    ///
    /// When a session_id is set and the EventBus is connected to a GlobalBus,
    /// all published events will be broadcast to the GlobalBus with this
    /// session ID as the source.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier
    pub fn with_session_id(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    /// Connect this EventBus to a GlobalBus for automatic event broadcast.
    ///
    /// When connected, all events published to this EventBus will automatically
    /// be broadcast to the GlobalBus using the configured agent_id and session_id.
    ///
    /// # Arguments
    ///
    /// * `global_bus` - Reference to the GlobalBus singleton
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aether_core::event::{EventBus, GlobalBus};
    ///
    /// let bus = EventBus::new()
    ///     .with_agent_id("agent-1")
    ///     .with_session_id("session-1")
    ///     .with_global_bus(GlobalBus::global());
    /// ```
    pub fn with_global_bus(mut self, global_bus: &'static GlobalBus) -> Self {
        self.global_bus = Some(global_bus);
        self
    }

    /// Get the configured agent ID.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// Get the configured session ID.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Check if this EventBus is connected to a GlobalBus.
    pub fn is_connected_to_global(&self) -> bool {
        self.global_bus.is_some()
    }

    /// Publish an event to all subscribers
    ///
    /// Returns the number of active subscribers that received the event.
    ///
    /// If this EventBus is connected to a GlobalBus (via `with_global_bus`),
    /// the event will also be automatically broadcast to the GlobalBus with
    /// the configured agent_id and session_id.
    pub async fn publish(&self, event: AetherEvent) -> usize {
        let timestamped = TimestampedEvent::new(event.clone());

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

        // Broadcast to GlobalBus if connected
        if let Some(global_bus) = self.global_bus {
            let agent_id = self.agent_id.as_deref().unwrap_or("");
            let session_id = self.session_id.as_deref().unwrap_or("");

            trace!(
                agent_id,
                session_id,
                event_type = ?event.event_type(),
                "Auto-broadcasting to GlobalBus"
            );

            global_bus.broadcast(agent_id, session_id, event).await;
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
    use crate::event::filter::EventFilter;
    use crate::event::types::{InputEvent, StopReason};
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    // ========================================================================
    // GlobalBus Integration Tests
    // ========================================================================

    #[test]
    fn test_event_bus_builder_methods() {
        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1");

        assert_eq!(bus.agent_id(), Some("agent-1"));
        assert_eq!(bus.session_id(), Some("session-1"));
        assert!(!bus.is_connected_to_global());
    }

    #[test]
    fn test_event_bus_with_global_bus() {
        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(GlobalBus::global());

        assert!(bus.is_connected_to_global());
        assert_eq!(bus.agent_id(), Some("agent-1"));
        assert_eq!(bus.session_id(), Some("session-1"));
    }

    #[test]
    fn test_event_bus_without_context() {
        let bus = EventBus::new();

        assert_eq!(bus.agent_id(), None);
        assert_eq!(bus.session_id(), None);
        assert!(!bus.is_connected_to_global());
    }

    #[tokio::test]
    async fn test_auto_broadcast_to_global_bus() {
        // Create a new GlobalBus for this test (not the singleton)
        // to avoid interference with other tests
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        // Track callback invocations
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to the GlobalBus
        let filter = EventFilter::all()
            .with_agent("test-agent")
            .with_session("test-session");

        let _sub_id = global_bus
            .subscribe_async(filter, move |event| {
                assert_eq!(event.source_agent_id, "test-agent");
                assert_eq!(event.source_session_id, "test-session");
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Create an EventBus connected to the GlobalBus
        let bus = EventBus::new()
            .with_agent_id("test-agent")
            .with_session_id("test-session")
            .with_global_bus(global_bus);

        // Publish an event
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify the GlobalBus received the event
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_no_broadcast_without_global_bus() {
        // Create an EventBus without GlobalBus connection
        let bus = EventBus::new()
            .with_agent_id("test-agent")
            .with_session_id("test-session");

        // This should not panic or cause any issues
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Just verify the local subscriber still works
        let mut subscriber = bus.subscribe();
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        let received = subscriber.try_recv().unwrap();
        assert!(received.is_some());
    }

    #[tokio::test]
    async fn test_broadcast_with_missing_context() {
        // Create a GlobalBus for this test
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        // Track callback invocations
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to all events (no agent/session filter)
        let filter = EventFilter::all();

        let _sub_id = global_bus
            .subscribe_async(filter, move |event| {
                // Should have empty strings for missing context
                assert!(event.source_agent_id.is_empty());
                assert!(event.source_session_id.is_empty());
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Create an EventBus with GlobalBus but no context
        let bus = EventBus::new().with_global_bus(global_bus);

        // Publish an event
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Should still broadcast (with empty agent/session)
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_event_buses_to_global() {
        // Create a GlobalBus for this test
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        // Track events from different agents using AtomicUsize for thread-safe counting
        let counter = Arc::new(AtomicUsize::new(0));
        let agent1_seen = Arc::new(AtomicUsize::new(0));
        let agent2_seen = Arc::new(AtomicUsize::new(0));

        let counter_clone = counter.clone();
        let agent1_clone = agent1_seen.clone();
        let agent2_clone = agent2_seen.clone();

        // Subscribe to all events
        let filter = EventFilter::all();

        let _sub_id = global_bus
            .subscribe_async(filter, move |event| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                if event.source_agent_id == "agent-1" {
                    agent1_clone.fetch_add(1, Ordering::SeqCst);
                }
                if event.source_agent_id == "agent-2" {
                    agent2_clone.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        // Create two EventBus instances connected to the same GlobalBus
        let bus1 = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        let bus2 = EventBus::new()
            .with_agent_id("agent-2")
            .with_session_id("session-2")
            .with_global_bus(global_bus);

        // Publish events from both buses
        bus1.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;
        bus2.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify both events were received with correct source info
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(agent1_seen.load(Ordering::SeqCst), 1);
        assert_eq!(agent2_seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_cloned_event_bus_preserves_global_bus() {
        // Create a GlobalBus for this test
        let global_bus = Box::leak(Box::new(GlobalBus::new()));

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let filter = EventFilter::all();
        let _sub_id = global_bus
            .subscribe_async(filter, move |_| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Create and clone an EventBus
        let bus = EventBus::new()
            .with_agent_id("agent-1")
            .with_session_id("session-1")
            .with_global_bus(global_bus);

        let bus_clone = bus.clone();

        // Both should preserve the GlobalBus connection
        assert!(bus.is_connected_to_global());
        assert!(bus_clone.is_connected_to_global());

        // Both should broadcast to GlobalBus
        bus.publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;
        bus_clone
            .publish(AetherEvent::LoopStop(StopReason::Completed))
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}

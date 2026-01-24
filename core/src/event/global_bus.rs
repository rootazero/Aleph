// Aether/core/src/event/global_bus.rs
//! Global event bus for cross-agent event aggregation.
//!
//! The `GlobalBus` provides a singleton event bus that aggregates events from
//! multiple Agent EventBus instances, enabling cross-agent event subscription
//! and routing.
//!
//! # Example
//!
//! ```rust,ignore
//! use aether_core::event::global_bus::GlobalBus;
//! use aether_core::event::filter::EventFilter;
//! use aether_core::event::EventType;
//!
//! // Access the global singleton
//! let bus = GlobalBus::global();
//!
//! // Subscribe to tool events from all agents
//! let filter = EventFilter::new(vec![
//!     EventType::ToolCallStarted,
//!     EventType::ToolCallCompleted,
//! ]);
//!
//! let sub_id = bus.subscribe(filter, |event| {
//!     println!("Received event from agent: {}", event.source_agent_id);
//! });
//!
//! // Later: unsubscribe
//! bus.unsubscribe(&sub_id).await;
//! ```

use crate::event::bus::EventBus;
use crate::event::filter::EventFilter;
use crate::event::types::AetherEvent;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, trace};

// =============================================================================
// Constants
// =============================================================================

/// Default buffer size for the global broadcast channel
const DEFAULT_BUFFER_SIZE: usize = 1024;

// =============================================================================
// GlobalEvent
// =============================================================================

/// Global event wrapper for cross-agent event routing.
///
/// Wraps an `AetherEvent` with source tracking metadata to enable
/// cross-agent event filtering and routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalEvent {
    /// The source agent that emitted this event
    pub source_agent_id: String,
    /// The source session that emitted this event
    pub source_session_id: String,
    /// The actual event payload
    pub event: AetherEvent,
    /// Timestamp when the event was emitted (epoch millis)
    pub timestamp: i64,
    /// Monotonic sequence number for ordering
    pub sequence: u64,
}

impl GlobalEvent {
    /// Create a new GlobalEvent with automatic timestamp and sequence.
    pub fn new(
        source_agent_id: impl Into<String>,
        source_session_id: impl Into<String>,
        event: AetherEvent,
        sequence: u64,
    ) -> Self {
        Self {
            source_agent_id: source_agent_id.into(),
            source_session_id: source_session_id.into(),
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence,
        }
    }

    /// Create a GlobalEvent for testing purposes (with zero sequence).
    #[cfg(test)]
    pub fn for_test(
        source_session_id: impl Into<String>,
        source_agent_id: Option<String>,
        event: AetherEvent,
    ) -> Self {
        Self {
            source_agent_id: source_agent_id.unwrap_or_default(),
            source_session_id: source_session_id.into(),
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence: 0,
        }
    }
}

// =============================================================================
// SubscriptionId
// =============================================================================

/// Unique identifier for a subscription.
pub type SubscriptionId = String;

// =============================================================================
// Subscription
// =============================================================================

/// A subscription to global events with filtering.
pub struct Subscription {
    /// Unique identifier for this subscription
    pub id: SubscriptionId,
    /// Filter to match events
    pub filter: EventFilter,
    /// Callback to invoke when matching events arrive
    pub callback: Arc<dyn Fn(GlobalEvent) + Send + Sync>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("filter", &self.filter)
            .field("callback", &"<callback>")
            .finish()
    }
}

// =============================================================================
// GlobalBus
// =============================================================================

/// Global event bus singleton for cross-agent event aggregation.
///
/// The GlobalBus aggregates events from multiple Agent EventBus instances,
/// enabling cross-agent event subscription. It uses a broadcast channel
/// internally and maintains weak references to registered agent buses.
pub struct GlobalBus {
    /// Broadcast sender for global events
    sender: broadcast::Sender<GlobalEvent>,
    /// Active subscriptions indexed by ID
    subscriptions: RwLock<HashMap<SubscriptionId, Subscription>>,
    /// Registered agent event buses (weak references to allow cleanup)
    agent_buses: RwLock<HashMap<String, Weak<EventBus>>>,
    /// Monotonic sequence counter
    sequence: AtomicU64,
}

// Singleton instance
static GLOBAL_BUS: Lazy<GlobalBus> = Lazy::new(GlobalBus::new);

impl GlobalBus {
    /// Create a new GlobalBus instance.
    ///
    /// Note: For most use cases, prefer `GlobalBus::global()` to access
    /// the singleton instance.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_BUFFER_SIZE);
        Self {
            sender,
            subscriptions: RwLock::new(HashMap::new()),
            agent_buses: RwLock::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }

    /// Get the global singleton instance.
    ///
    /// This is the preferred way to access the GlobalBus.
    pub fn global() -> &'static GlobalBus {
        &GLOBAL_BUS
    }

    /// Register an agent's event bus.
    ///
    /// The GlobalBus maintains a weak reference to the EventBus,
    /// allowing the agent to be dropped without preventing cleanup.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - Unique identifier for the agent
    /// * `bus` - The agent's EventBus instance
    pub async fn register_agent(&self, agent_id: &str, bus: Arc<EventBus>) {
        let mut buses = self.agent_buses.write().await;
        buses.insert(agent_id.to_string(), Arc::downgrade(&bus));
        debug!(agent_id, "Registered agent event bus");
    }

    /// Unregister an agent's event bus.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The agent ID to unregister
    pub async fn unregister_agent(&self, agent_id: &str) {
        let mut buses = self.agent_buses.write().await;
        if buses.remove(agent_id).is_some() {
            debug!(agent_id, "Unregistered agent event bus");
        }
    }

    /// Broadcast an event to all matching subscribers.
    ///
    /// Creates a GlobalEvent and notifies all subscribers whose filters
    /// match the event.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The source agent ID
    /// * `session_id` - The source session ID
    /// * `event` - The event to broadcast
    pub async fn broadcast(&self, agent_id: &str, session_id: &str, event: AetherEvent) {
        let sequence = self.next_sequence();
        let global_event = GlobalEvent::new(agent_id, session_id, event, sequence);

        trace!(
            agent_id,
            session_id,
            sequence,
            event_type = ?global_event.event.event_type(),
            "Broadcasting global event"
        );

        // Send via broadcast channel for async subscribers
        if let Err(e) = self.sender.send(global_event.clone()) {
            trace!("No broadcast receivers: {}", e);
        }

        // Notify callback-based subscribers
        let subscriptions = self.subscriptions.read().await;
        for subscription in subscriptions.values() {
            if subscription.filter.matches(&global_event) {
                (subscription.callback)(global_event.clone());
            }
        }
    }

    /// Subscribe to global events with a filter and callback.
    ///
    /// Returns a subscription ID that can be used to unsubscribe later.
    ///
    /// # Arguments
    ///
    /// * `filter` - Event filter to match events
    /// * `callback` - Function to call when matching events arrive
    ///
    /// # Returns
    ///
    /// A unique subscription ID
    pub fn subscribe(
        &self,
        filter: EventFilter,
        callback: impl Fn(GlobalEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = uuid::Uuid::new_v4().to_string();
        let subscription = Subscription {
            id: id.clone(),
            filter,
            callback: Arc::new(callback),
        };

        // Use blocking write since this is called from sync context
        // In async context, use subscribe_async instead
        let subscriptions = self.subscriptions.blocking_write();
        let mut subs = subscriptions;
        subs.insert(id.clone(), subscription);

        debug!(subscription_id = %id, "Added global event subscription");
        id
    }

    /// Subscribe to global events (async version).
    ///
    /// Identical to `subscribe` but can be called from async context.
    pub async fn subscribe_async(
        &self,
        filter: EventFilter,
        callback: impl Fn(GlobalEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = uuid::Uuid::new_v4().to_string();
        let subscription = Subscription {
            id: id.clone(),
            filter,
            callback: Arc::new(callback),
        };

        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.insert(id.clone(), subscription);

        debug!(subscription_id = %id, "Added global event subscription (async)");
        id
    }

    /// Unsubscribe from global events.
    ///
    /// # Arguments
    ///
    /// * `id` - The subscription ID returned from `subscribe`
    pub async fn unsubscribe(&self, id: &SubscriptionId) {
        let mut subscriptions = self.subscriptions.write().await;
        if subscriptions.remove(id).is_some() {
            debug!(subscription_id = %id, "Removed global event subscription");
        }
    }

    /// Get the next sequence number.
    ///
    /// Returns a monotonically increasing sequence number for ordering events.
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    /// Get a broadcast receiver for async event handling.
    ///
    /// This is useful for components that want to process events
    /// asynchronously using `recv().await`.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<GlobalEvent> {
        self.sender.subscribe()
    }

    /// Get the current number of registered agents.
    pub async fn agent_count(&self) -> usize {
        let buses = self.agent_buses.read().await;
        buses.len()
    }

    /// Get the current number of active subscriptions.
    pub async fn subscription_count(&self) -> usize {
        let subscriptions = self.subscriptions.read().await;
        subscriptions.len()
    }

    /// Clean up stale agent references.
    ///
    /// Removes weak references to EventBus instances that have been dropped.
    pub async fn cleanup_stale_agents(&self) {
        let mut buses = self.agent_buses.write().await;
        let stale_ids: Vec<String> = buses
            .iter()
            .filter(|(_, weak)| weak.strong_count() == 0)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale_ids {
            buses.remove(id);
            debug!(agent_id = %id, "Cleaned up stale agent reference");
        }

        if !stale_ids.is_empty() {
            debug!(count = stale_ids.len(), "Cleaned up stale agent references");
        }
    }
}

impl Default for GlobalBus {
    fn default() -> Self {
        Self::new()
    }
}

// GlobalBus is Send + Sync due to the use of tokio::sync::RwLock and AtomicU64.
// The broadcast::Sender is also Send + Sync when the item type (GlobalEvent) is Send.

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};
    use crate::event::EventType;
    use std::sync::atomic::AtomicUsize;

    fn make_input_event() -> AetherEvent {
        AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })
    }

    fn make_loop_stop_event() -> AetherEvent {
        AetherEvent::LoopStop(StopReason::Completed)
    }

    #[test]
    fn test_singleton_access() {
        // Access singleton multiple times and verify it's the same instance
        let bus1 = GlobalBus::global();
        let bus2 = GlobalBus::global();

        // Same pointer means same instance
        assert!(std::ptr::eq(bus1, bus2));
    }

    #[test]
    fn test_global_event_creation() {
        let event = GlobalEvent::new("agent-1", "session-1", make_input_event(), 42);

        assert_eq!(event.source_agent_id, "agent-1");
        assert_eq!(event.source_session_id, "session-1");
        assert_eq!(event.sequence, 42);
        assert!(event.timestamp > 0);
    }

    #[test]
    fn test_sequence_increment() {
        let bus = GlobalBus::new();

        let seq1 = bus.next_sequence();
        let seq2 = bus.next_sequence();
        let seq3 = bus.next_sequence();

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
        assert_eq!(seq3, 2);
    }

    #[tokio::test]
    async fn test_agent_registration() {
        let bus = GlobalBus::new();
        let event_bus = Arc::new(EventBus::new());

        // Register agent
        bus.register_agent("agent-1", event_bus.clone()).await;
        assert_eq!(bus.agent_count().await, 1);

        // Register another agent
        let event_bus2 = Arc::new(EventBus::new());
        bus.register_agent("agent-2", event_bus2).await;
        assert_eq!(bus.agent_count().await, 2);

        // Unregister
        bus.unregister_agent("agent-1").await;
        assert_eq!(bus.agent_count().await, 1);

        // Unregister non-existent (should not panic)
        bus.unregister_agent("agent-3").await;
        assert_eq!(bus.agent_count().await, 1);
    }

    #[tokio::test]
    async fn test_broadcast_to_matching_subscribers() {
        let bus = GlobalBus::new();

        // Counter to track callback invocations
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to InputReceived events
        let filter = EventFilter::new(vec![EventType::InputReceived]);
        let _sub_id = bus
            .subscribe_async(filter, move |_event| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast matching event
        bus.broadcast("agent-1", "session-1", make_input_event())
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Broadcast non-matching event
        bus.broadcast("agent-1", "session-1", make_loop_stop_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Counter should still be 1 (non-matching event)
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_subscribe_unsubscribe() {
        let bus = GlobalBus::new();

        // Subscribe
        let filter = EventFilter::all();
        let sub_id = bus.subscribe_async(filter, |_| {}).await;

        assert_eq!(bus.subscription_count().await, 1);

        // Unsubscribe
        bus.unsubscribe(&sub_id).await;
        assert_eq!(bus.subscription_count().await, 0);

        // Unsubscribe non-existent (should not panic)
        bus.unsubscribe(&"non-existent".to_string()).await;
    }

    #[tokio::test]
    async fn test_filter_by_agent() {
        let bus = GlobalBus::new();

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to events from agent-1 only
        let filter = EventFilter::all().with_agent("agent-1");
        let _sub_id = bus
            .subscribe_async(filter, move |_event| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast from agent-1
        bus.broadcast("agent-1", "session-1", make_input_event())
            .await;

        // Broadcast from agent-2 (should not match)
        bus.broadcast("agent-2", "session-2", make_input_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Only one event should have matched
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_filter_by_session() {
        let bus = GlobalBus::new();

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to events from session-1 only
        let filter = EventFilter::all().with_session("session-1");
        let _sub_id = bus
            .subscribe_async(filter, move |_event| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast from session-1
        bus.broadcast("agent-1", "session-1", make_input_event())
            .await;

        // Broadcast from session-2 (should not match)
        bus.broadcast("agent-1", "session-2", make_input_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Only one event should have matched
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = GlobalBus::new();

        let counter1 = Arc::new(AtomicUsize::new(0));
        let counter1_clone = counter1.clone();

        let counter2 = Arc::new(AtomicUsize::new(0));
        let counter2_clone = counter2.clone();

        // Subscribe two subscribers
        let filter1 = EventFilter::new(vec![EventType::InputReceived]);
        let _sub1 = bus
            .subscribe_async(filter1, move |_event| {
                counter1_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        let filter2 = EventFilter::all();
        let _sub2 = bus
            .subscribe_async(filter2, move |_event| {
                counter2_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast event
        bus.broadcast("agent-1", "session-1", make_input_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Both subscribers should receive the event
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_cleanup_stale_agents() {
        let bus = GlobalBus::new();

        // Create and register an agent
        {
            let event_bus = Arc::new(EventBus::new());
            bus.register_agent("agent-temp", event_bus).await;
            assert_eq!(bus.agent_count().await, 1);
            // event_bus is dropped here
        }

        // Cleanup should remove the stale reference
        bus.cleanup_stale_agents().await;
        assert_eq!(bus.agent_count().await, 0);
    }

    #[tokio::test]
    async fn test_broadcast_receiver() {
        let bus = GlobalBus::new();

        let mut receiver = bus.subscribe_broadcast();

        // Broadcast event
        bus.broadcast("agent-1", "session-1", make_input_event())
            .await;

        // Receive via broadcast channel
        let received = tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            receiver.recv(),
        )
        .await;

        assert!(received.is_ok());
        let event = received.unwrap().unwrap();
        assert_eq!(event.source_agent_id, "agent-1");
        assert_eq!(event.source_session_id, "session-1");
    }

    #[test]
    fn test_global_event_serialization() {
        let event = GlobalEvent::new("agent-1", "session-1", make_input_event(), 123);

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("agent-1"));
        assert!(json.contains("session-1"));
        assert!(json.contains("123"));

        let parsed: GlobalEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source_agent_id, "agent-1");
        assert_eq!(parsed.source_session_id, "session-1");
        assert_eq!(parsed.sequence, 123);
    }
}

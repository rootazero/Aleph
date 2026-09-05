// Aleph/core/src/event/global_bus.rs
//! Global event bus for cross-agent event aggregation.
//!
//! The `GlobalBus` provides a singleton event bus that aggregates events from
//! multiple Agent `EventBus` instances, enabling cross-agent event subscription
//! and routing.
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::event::global_bus::GlobalBus;
//! use alephcore::event::filter::EventFilter;
//! use alephcore::event::EventType;
//!
//! // Access the global singleton
//! let bus = GlobalBus::global();
//!
//! // Subscribe to tool events from all agents (async — the canonical entry point)
//! let filter = EventFilter::new(vec![
//!     EventType::ProcessCompleted,
//! ]);
//!
//! let sub_id = bus.subscribe_async(filter, |event| {
//!     println!("Received event from agent: {}", event.source_agent_id);
//! }).await;
//!
//! // Later: unsubscribe
//! bus.unsubscribe(&sub_id).await;
//! ```

use crate::event::filter::EventFilter;
use crate::event::types::AlephEvent;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AsyncRwLock, AtomicU64, Ordering};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::broadcast;
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
/// Wraps an `AlephEvent` with source tracking metadata to enable
/// cross-agent event filtering and routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalEvent {
    /// The source agent that emitted this event
    pub source_agent_id: String,
    /// The source session that emitted this event
    pub source_session_id: String,
    /// The actual event payload
    pub event: AlephEvent,
    /// Timestamp when the event was emitted (epoch millis)
    pub timestamp: i64,
    /// Monotonic sequence number for ordering
    pub sequence: u64,
}

impl GlobalEvent {
    /// Create a new `GlobalEvent` with automatic timestamp and sequence.
    pub fn new(
        source_agent_id: impl Into<String>,
        source_session_id: impl Into<String>,
        event: AlephEvent,
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
        event: AlephEvent,
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

/// Unique identifier for a subscription (newtype for type safety)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionId(String);

impl SubscriptionId {
    /// Create a new `SubscriptionId`
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

// NOTE: `as_str` / `into_inner` / `Deref` / `From<String>` / `From<&str>`
// were removed in the 2026-09 severed-wire review — zero callers anywhere in
// the workspace. `Display` is the only accessor left because the tracing
// macros render the id via `%id`.
impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// Subscription
// =============================================================================

/// A subscription to global events with filtering.
///
/// Private to `global_bus`: no public API exposes a `Subscription` (only
/// [`SubscriptionId`] crosses the module boundary), so the previous
/// `pub`-with-`pub`-fields shape — kept, per its own comment, "until the
/// visibility is tightened in one patch" — is tightened here (2026-09
/// severed-wire review). The struct was also re-exported as
/// `crate::event::Subscription` with zero callers; that re-export is gone.
struct Subscription {
    /// Unique identifier for this subscription
    id: SubscriptionId,
    /// Filter to match events
    filter: EventFilter,
    /// Callback to invoke when matching events arrive
    callback: Arc<dyn Fn(GlobalEvent) + Send + Sync>,
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
/// The `GlobalBus` aggregates events from multiple Agent `EventBus` instances,
/// enabling cross-agent event subscription. It uses a broadcast channel
/// internally.
pub struct GlobalBus {
    /// Broadcast sender for global events
    sender: broadcast::Sender<GlobalEvent>,
    /// Active subscriptions indexed by ID
    subscriptions: AsyncRwLock<HashMap<SubscriptionId, Subscription>>,
    /// Monotonic sequence counter
    sequence: AtomicU64,
}

// Singleton instance
static GLOBAL_BUS: Lazy<GlobalBus> = Lazy::new(GlobalBus::new);

impl Default for GlobalBus {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalBus {
    /// Create a new `GlobalBus` instance.
    ///
    /// Note: For most use cases, prefer `GlobalBus::global()` to access
    /// the singleton instance.
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_BUFFER_SIZE);
        Self {
            sender,
            subscriptions: AsyncRwLock::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }

    /// Get the global singleton instance.
    ///
    /// This is the preferred way to access the `GlobalBus`.
    #[must_use]
    pub fn global() -> &'static Self {
        &GLOBAL_BUS
    }

    /// Broadcast an event to all matching subscribers.
    ///
    /// Creates a `GlobalEvent` and notifies all subscribers whose filters
    /// match the event.
    ///
    /// # Callback dispatch contract
    ///
    /// Delivery to a single subscriber is **not ordered**: each matching
    /// callback is dispatched in its own `tokio::spawn`, so two consecutive
    /// broadcasts may reach the same callback out of order. Consumers that
    /// need ordering must sort on [`GlobalEvent::sequence`].
    ///
    /// The same applies to [`subscribe_broadcast`](Self::subscribe_broadcast)
    /// receivers on the shared channel — two concurrent broadcasts whose
    /// `send` calls interleave on the channel can arrive out of order. Sort
    /// by `sequence` before consuming the stream.
    ///
    /// # Single-delivery contract
    ///
    /// A subscriber that holds BOTH a [`broadcast::Receiver`] (via
    /// [`subscribe_broadcast`](Self::subscribe_broadcast)) AND a callback
    /// (via [`subscribe_async`](Self::subscribe_async)) will receive every
    /// event **twice** — once from the channel send below, once from the
    /// callback dispatch. Pick one path per subscriber.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The source agent ID
    /// * `session_id` - The source session ID
    /// * `event` - The event to broadcast
    pub async fn broadcast(&self, agent_id: &str, session_id: &str, event: AlephEvent) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        let global_event = GlobalEvent::new(agent_id, session_id, event, sequence);

        trace!(
            agent_id,
            session_id,
            sequence,
            event_type = ?global_event.event.event_type(),
            "Broadcasting global event"
        );

        // Send via broadcast channel for async subscribers. A subscriber
        // who ALSO wired `subscribe_async` for the same event will see it
        // twice — see the "Single-delivery contract" doc above.
        if let Err(e) = self.sender.send(global_event.clone()) {
            trace!("No broadcast receivers: {}", e);
        }

        // Collect matching callbacks and dispatch each in its own task.
        //
        // Contract: callbacks must be non-blocking. `tokio::spawn` does NOT
        // make a blocking closure safe — it merely moves the blockage onto
        // another runtime worker. Subscribers that need to do async work
        // must use the extract-and-respawn pattern: pull the data out of
        // the event synchronously, then spawn their own task (see the
        // `goal_wait` subscriber in `gateway::execution_engine::goal_wait`).
        let callbacks = {
            let subscriptions = self.subscriptions.read().await;
            subscriptions
                .values()
                .filter(|sub| sub.filter.matches(&global_event))
                .map(|sub| Arc::clone(&sub.callback))
                .collect::<Vec<_>>()
        };

        for callback in callbacks {
            let event = global_event.clone();
            tokio::spawn(async move {
                callback(event);
            });
        }
    }

    /// Subscribe to global events (async version).
    ///
    /// This is the canonical subscription entry point. The callback is a
    /// synchronous closure dispatched per event inside its own
    /// `tokio::spawn`; see [`broadcast`](Self::broadcast) for the dispatch
    /// contract (non-blocking requirement, no per-subscriber ordering).
    pub async fn subscribe_async(
        &self,
        filter: EventFilter,
        callback: impl Fn(GlobalEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = SubscriptionId::new(uuid::Uuid::new_v4().to_string());
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

    /// Get a broadcast receiver for async event handling.
    ///
    /// This is useful for components that want to process events
    /// asynchronously using `recv().await`.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<GlobalEvent> {
        self.sender.subscribe()
    }

    /// Get the current number of active subscriptions.
    #[cfg(test)]
    pub async fn subscription_count(&self) -> usize {
        let subscriptions = self.subscriptions.read().await;
        subscriptions.len()
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
    use crate::event::types::{ProcessCompletionEvent, SubAgentCompletionEvent};
    use crate::event::EventType;
    use crate::sync_primitives::AtomicUsize;

    fn make_subagent_event() -> AlephEvent {
        AlephEvent::SubAgentCompleted(SubAgentCompletionEvent {
            agent_id: "a".into(),
            child_session_id: "s".into(),
            summary: "done".into(),
            success: true,
            error: None,
            request_id: None,
            request_ids: Vec::new(),
        })
    }

    fn make_process_event() -> AlephEvent {
        AlephEvent::ProcessCompleted(ProcessCompletionEvent {
            process_id: 1,
            command: "echo".into(),
            exit_code: 0,
            success: true,
            output_tail: "ok".into(),
            output_truncated: false,
        })
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
        let event = GlobalEvent::new("agent-1", "session-1", make_subagent_event(), 42);

        assert_eq!(event.source_agent_id, "agent-1");
        assert_eq!(event.source_session_id, "session-1");
        assert_eq!(event.sequence, 42);
        assert!(event.timestamp > 0);
    }

    #[tokio::test]
    async fn test_broadcast_to_matching_subscribers() {
        let bus = GlobalBus::new();

        // Counter to track callback invocations
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        // Subscribe to SubAgentCompleted events
        let filter = EventFilter::new(vec![EventType::SubAgentCompleted]);
        let _sub_id = bus
            .subscribe_async(filter, move |_event| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Broadcast matching event
        bus.broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        // Allow async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Broadcast non-matching event
        bus.broadcast("agent-1", "session-1", make_process_event())
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
        bus.unsubscribe(&SubscriptionId::new("non-existent")).await;
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
        bus.broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        // Broadcast from agent-2 (should not match)
        bus.broadcast("agent-2", "session-2", make_subagent_event())
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
        bus.broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        // Broadcast from session-2 (should not match)
        bus.broadcast("agent-1", "session-2", make_subagent_event())
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
        let filter1 = EventFilter::new(vec![EventType::SubAgentCompleted]);
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
        bus.broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Both subscribers should receive the event
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_broadcast_receiver() {
        let bus = GlobalBus::new();

        let mut receiver = bus.subscribe_broadcast();

        // Broadcast event
        bus.broadcast("agent-1", "session-1", make_subagent_event())
            .await;

        // Receive via broadcast channel
        let received =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), receiver.recv()).await;

        assert!(received.is_ok());
        let event = received.unwrap().unwrap();
        assert_eq!(event.source_agent_id, "agent-1");
        assert_eq!(event.source_session_id, "session-1");
    }

    #[test]
    fn test_global_event_serialization() {
        let event = GlobalEvent::new("agent-1", "session-1", make_subagent_event(), 123);

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

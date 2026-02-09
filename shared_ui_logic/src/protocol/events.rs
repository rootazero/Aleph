//! Event dispatching system

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Event callback type
///
/// A callback function that receives an event payload.
pub type EventCallback = Arc<dyn Fn(Value) + Send + Sync>;

/// Event dispatcher for pub/sub pattern
///
/// The dispatcher allows subscribing to specific event topics
/// and dispatching events to all subscribers.
///
/// ## Features
///
/// - Topic-based subscription
/// - Wildcard subscription (`*` receives all events)
/// - Multiple subscribers per topic
/// - Thread-safe (Send + Sync)
///
/// ## Example
///
/// ```rust
/// use aleph_ui_logic::protocol::EventDispatcher;
///
/// #[tokio::main]
/// async fn main() {
///     let dispatcher = EventDispatcher::new();
///
///     // Subscribe to specific topic
///     dispatcher.subscribe("agent.thinking", |payload| {
///         println!("Agent thinking: {:?}", payload);
///     }).await;
///
///     // Subscribe to all events
///     dispatcher.subscribe("*", |payload| {
///         println!("Event: {:?}", payload);
///     }).await;
///
///     // Dispatch event
///     dispatcher.dispatch("agent.thinking", serde_json::json!({
///         "content": "Analyzing the problem..."
///     })).await;
/// }
/// ```
pub struct EventDispatcher {
    handlers: Arc<RwLock<HashMap<String, Vec<EventCallback>>>>,
}

impl EventDispatcher {
    /// Create a new event dispatcher
    ///
    /// # Example
    ///
    /// ```rust
    /// use aleph_ui_logic::protocol::EventDispatcher;
    ///
    /// let dispatcher = EventDispatcher::new();
    /// ```
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Subscribe to an event topic
    ///
    /// # Arguments
    ///
    /// - `topic`: The event topic to subscribe to (use `"*"` for all events)
    /// - `callback`: The callback function to invoke when an event is received
    ///
    /// # Example
    ///
    /// ```rust
    /// # use aleph_ui_logic::protocol::EventDispatcher;
    /// # #[tokio::main]
    /// # async fn main() {
    /// let dispatcher = EventDispatcher::new();
    ///
    /// dispatcher.subscribe("agent.thinking", |payload| {
    ///     println!("Received: {:?}", payload);
    /// }).await;
    /// # }
    /// ```
    pub async fn subscribe<F>(&self, topic: &str, callback: F)
    where
        F: Fn(Value) + Send + Sync + 'static,
    {
        let mut handlers = self.handlers.write().await;
        handlers
            .entry(topic.to_string())
            .or_insert_with(Vec::new)
            .push(Arc::new(callback));
    }

    /// Unsubscribe from an event topic
    ///
    /// This removes all callbacks for the specified topic.
    ///
    /// # Arguments
    ///
    /// - `topic`: The event topic to unsubscribe from
    ///
    /// # Example
    ///
    /// ```rust
    /// # use aleph_ui_logic::protocol::EventDispatcher;
    /// # #[tokio::main]
    /// # async fn main() {
    /// let dispatcher = EventDispatcher::new();
    ///
    /// dispatcher.subscribe("test.event", |_| {}).await;
    /// dispatcher.unsubscribe("test.event").await;
    /// # }
    /// ```
    pub async fn unsubscribe(&self, topic: &str) {
        let mut handlers = self.handlers.write().await;
        handlers.remove(topic);
    }

    /// Dispatch an event to all subscribers
    ///
    /// This will invoke all callbacks subscribed to the specific topic
    /// as well as all wildcard (`*`) subscribers.
    ///
    /// # Arguments
    ///
    /// - `topic`: The event topic
    /// - `payload`: The event payload
    ///
    /// # Example
    ///
    /// ```rust
    /// # use aleph_ui_logic::protocol::EventDispatcher;
    /// # #[tokio::main]
    /// # async fn main() {
    /// let dispatcher = EventDispatcher::new();
    ///
    /// dispatcher.dispatch("agent.thinking", serde_json::json!({
    ///     "content": "Processing..."
    /// })).await;
    /// # }
    /// ```
    pub async fn dispatch(&self, topic: &str, payload: Value) {
        let handlers = self.handlers.read().await;

        // Dispatch to specific topic subscribers
        if let Some(callbacks) = handlers.get(topic) {
            for callback in callbacks {
                callback(payload.clone());
            }
        }

        // Dispatch to wildcard subscribers
        if let Some(callbacks) = handlers.get("*") {
            for callback in callbacks {
                callback(payload.clone());
            }
        }
    }

    /// Get the number of subscribers for a topic
    ///
    /// # Arguments
    ///
    /// - `topic`: The event topic
    ///
    /// # Returns
    ///
    /// The number of subscribers for the topic
    pub async fn subscriber_count(&self, topic: &str) -> usize {
        let handlers = self.handlers.read().await;
        handlers.get(topic).map(|v| v.len()).unwrap_or(0)
    }

    /// Get all subscribed topics
    ///
    /// # Returns
    ///
    /// A vector of all topics that have at least one subscriber
    pub async fn topics(&self) -> Vec<String> {
        let handlers = self.handlers.read().await;
        handlers.keys().cloned().collect()
    }

    /// Clear all subscriptions
    ///
    /// This removes all callbacks for all topics.
    pub async fn clear(&self) {
        let mut handlers = self.handlers.write().await;
        handlers.clear();
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for EventDispatcher {
    fn clone(&self) -> Self {
        Self {
            handlers: Arc::clone(&self.handlers),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_subscribe_and_dispatch() {
        let dispatcher = EventDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        dispatcher
            .subscribe("test.event", move |_| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        dispatcher
            .dispatch("test.event", serde_json::json!({"test": "data"}))
            .await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_wildcard_subscription() {
        let dispatcher = EventDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        dispatcher
            .subscribe("*", move |_| {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        dispatcher
            .dispatch("any.event", serde_json::json!({}))
            .await;
        dispatcher
            .dispatch("another.event", serde_json::json!({}))
            .await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let dispatcher = EventDispatcher::new();

        dispatcher.subscribe("test.event", |_| {}).await;
        assert_eq!(dispatcher.subscriber_count("test.event").await, 1);

        dispatcher.unsubscribe("test.event").await;
        assert_eq!(dispatcher.subscriber_count("test.event").await, 0);
    }

    #[tokio::test]
    async fn test_topics() {
        let dispatcher = EventDispatcher::new();

        dispatcher.subscribe("topic1", |_| {}).await;
        dispatcher.subscribe("topic2", |_| {}).await;

        let topics = dispatcher.topics().await;
        assert_eq!(topics.len(), 2);
        assert!(topics.contains(&"topic1".to_string()));
        assert!(topics.contains(&"topic2".to_string()));
    }

    #[tokio::test]
    async fn test_clear() {
        let dispatcher = EventDispatcher::new();

        dispatcher.subscribe("topic1", |_| {}).await;
        dispatcher.subscribe("topic2", |_| {}).await;

        dispatcher.clear().await;
        assert_eq!(dispatcher.topics().await.len(), 0);
    }
}

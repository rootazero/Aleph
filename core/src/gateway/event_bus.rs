//! Gateway Event Bus
//!
//! Provides a broadcast channel for pushing events to all connected WebSocket clients.
//! Events are JSON-RPC 2.0 notifications (requests without an id).
//!
//! # Topic-Based Subscriptions
//!
//! The event bus supports topic-based filtering using glob-like patterns:
//! - `*` matches any single segment
//! - `**` or `*` at the end matches any remaining segments
//!
//! Examples:
//! - `agent.run.*` matches `agent.run.started`, `agent.run.completed`
//! - `session.*` matches `session.created`, `session.updated`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::debug;

/// Default channel capacity for event broadcasting
const EVENT_CHANNEL_SIZE: usize = 1024;

/// A topic-aware event that can be filtered by subscribers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicEvent {
    /// Event topic (e.g., "agent.run.started", "session.created")
    pub topic: String,
    /// Event payload
    pub data: Value,
    /// Timestamp (milliseconds since epoch)
    #[serde(default)]
    pub timestamp: u64,
}

impl TopicEvent {
    /// Create a new topic event
    pub fn new(topic: impl Into<String>, data: Value) -> Self {
        Self {
            topic: topic.into(),
            data,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Convert to JSON-RPC notification format
    pub fn to_notification(&self) -> Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": null,
            "params": {
                "topic": self.topic,
                "data": self.data,
                "timestamp": self.timestamp
            }
        })
    }
}

/// Check if a topic matches a pattern
///
/// Patterns support:
/// - Exact match: `agent.run.started`
/// - Single segment wildcard: `agent.*.started` matches `agent.run.started`
/// - Trailing wildcard: `agent.*` matches `agent.run`, `agent.run.started`
///
/// # Examples
///
/// ```ignore
/// assert!(topic_matches("agent.run.started", "agent.run.started"));
/// assert!(topic_matches("agent.run.started", "agent.run.*"));
/// assert!(topic_matches("agent.run.started", "agent.*"));
/// assert!(topic_matches("agent.run.started", "*"));
/// assert!(!topic_matches("agent.run.started", "session.*"));
/// ```
pub fn topic_matches(topic: &str, pattern: &str) -> bool {
    // Wildcard matches everything
    if pattern == "*" || pattern == "**" {
        return true;
    }

    let topic_parts: Vec<&str> = topic.split('.').collect();
    let pattern_parts: Vec<&str> = pattern.split('.').collect();

    let mut topic_idx = 0;
    let mut pattern_idx = 0;

    while pattern_idx < pattern_parts.len() && topic_idx < topic_parts.len() {
        let pattern_part = pattern_parts[pattern_idx];

        if pattern_part == "**" || (pattern_part == "*" && pattern_idx == pattern_parts.len() - 1) {
            // Trailing wildcard matches rest
            return true;
        }

        if pattern_part == "*" {
            // Single segment wildcard
            topic_idx += 1;
            pattern_idx += 1;
        } else if pattern_part == topic_parts[topic_idx] {
            // Exact match
            topic_idx += 1;
            pattern_idx += 1;
        } else {
            return false;
        }
    }

    // Both must be exhausted for a match (unless pattern ends with wildcard)
    topic_idx == topic_parts.len() && pattern_idx == pattern_parts.len()
}

/// A subscription filter for topic-based events
#[derive(Debug, Clone)]
pub struct TopicFilter {
    patterns: Vec<String>,
}

impl TopicFilter {
    /// Create a filter that matches all events
    pub fn all() -> Self {
        Self {
            patterns: vec!["*".to_string()],
        }
    }

    /// Create a filter with specific patterns
    pub fn with_patterns(patterns: Vec<String>) -> Self {
        Self { patterns }
    }

    /// Check if a topic matches any pattern in this filter
    pub fn matches(&self, topic: &str) -> bool {
        self.patterns.iter().any(|p| topic_matches(topic, p))
    }

    /// Add a pattern to the filter
    pub fn add_pattern(&mut self, pattern: impl Into<String>) {
        self.patterns.push(pattern.into());
    }

    /// Remove a pattern from the filter
    pub fn remove_pattern(&mut self, pattern: &str) -> bool {
        let initial_len = self.patterns.len();
        self.patterns.retain(|p| p != pattern);
        self.patterns.len() < initial_len
    }

    /// Get all patterns
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

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

    // Topic matching tests
    #[test]
    fn test_topic_exact_match() {
        assert!(topic_matches("agent.run.started", "agent.run.started"));
        assert!(!topic_matches("agent.run.started", "agent.run.completed"));
    }

    #[test]
    fn test_topic_wildcard_all() {
        assert!(topic_matches("agent.run.started", "*"));
        assert!(topic_matches("session.created", "*"));
        assert!(topic_matches("any.topic.here", "**"));
    }

    #[test]
    fn test_topic_trailing_wildcard() {
        assert!(topic_matches("agent.run.started", "agent.*"));
        assert!(topic_matches("agent.run", "agent.*"));
        assert!(topic_matches("agent.run.started", "agent.run.*"));
        assert!(!topic_matches("session.created", "agent.*"));
    }

    #[test]
    fn test_topic_single_segment_wildcard() {
        assert!(topic_matches("agent.run.started", "agent.*.started"));
        assert!(topic_matches("agent.task.started", "agent.*.started"));
        assert!(!topic_matches("agent.run.completed", "agent.*.started"));
    }

    #[test]
    fn test_topic_filter() {
        let filter = TopicFilter::with_patterns(vec![
            "agent.run.*".to_string(),
            "session.*".to_string(),
        ]);

        assert!(filter.matches("agent.run.started"));
        assert!(filter.matches("agent.run.completed"));
        assert!(filter.matches("session.created"));
        assert!(!filter.matches("config.updated"));
    }

    #[test]
    fn test_topic_filter_all() {
        let filter = TopicFilter::all();
        assert!(filter.matches("anything"));
        assert!(filter.matches("any.nested.topic"));
    }

    #[test]
    fn test_topic_event_notification() {
        let event = TopicEvent::new("agent.run.started", serde_json::json!({"run_id": "123"}));
        let notification = event.to_notification();

        assert!(notification.get("jsonrpc").is_some());
        assert!(notification.get("params").is_some());

        let params = notification.get("params").unwrap();
        assert_eq!(params.get("topic").unwrap().as_str().unwrap(), "agent.run.started");
    }
}

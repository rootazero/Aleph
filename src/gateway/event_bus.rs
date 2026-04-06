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

use super::events::GatewayEventFrame;

/// Default channel capacity for event broadcasting
const EVENT_CHANNEL_SIZE: usize = 1024;

/// Configuration changed event
#[derive(Debug, Clone, Serialize)]
pub struct ConfigChangedEvent {
    pub section: Option<String>,
    pub value: Value,
    pub timestamp: i64,
}

/// Gateway Event types
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum GatewayEvent {
    ConfigChanged(ConfigChangedEvent),
}

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
/// The event bus uses two broadcast channels internally:
/// - A string channel for backward compatibility (`subscribe()`)
/// - A typed channel for `GatewayEventFrame` (`subscribe_typed()`)
///
/// Events published via `publish()` go to both channels simultaneously.
pub struct GatewayEventBus {
    sender: broadcast::Sender<String>,
    typed_sender: broadcast::Sender<GatewayEventFrame>,
}

impl GatewayEventBus {
    /// Create a new event bus with default channel size
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        let (typed_sender, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        Self {
            sender,
            typed_sender,
        }
    }

    /// Create a new event bus with custom channel size
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (typed_sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            typed_sender,
        }
    }

    /// Publish a typed event frame to both the typed and string channels.
    ///
    /// Serializes the frame to JSON and sends to both the typed channel
    /// (for `subscribe_typed()`) and the string channel (for `subscribe()`).
    pub fn publish_frame(&self, frame: &GatewayEventFrame) -> Result<usize, serde_json::Error> {
        let preview = format!("{:?}", frame);
        debug!(
            "Publishing typed event: {}",
            &preview[..preview.len().min(100)]
        );
        let json = serde_json::to_string(frame)?;
        let typed_count = self.typed_sender.send(frame.clone()).unwrap_or(0);
        let str_count = self.sender.send(json).unwrap_or(0);
        Ok(typed_count.max(str_count))
    }

    /// Publish a raw JSON string event to the string channel.
    ///
    /// For new code, prefer `publish_frame()` to get typed channel support.
    pub fn publish(&self, event: impl AsRef<str>) -> usize {
        let event_str = event.as_ref();
        let preview = if event_str.chars().count() > 100 {
            let truncated: String = event_str.chars().take(100).collect();
            format!("{}...", truncated)
        } else {
            event_str.to_string()
        };
        debug!("Publishing event: {}", preview);
        self.sender.send(event_str.to_string()).unwrap_or(0)
    }

    /// Publish a typed event by serializing it to JSON.
    pub fn publish_json<T: serde::Serialize>(&self, event: &T) -> Result<usize, serde_json::Error> {
        let json = serde_json::to_string(event)?;
        Ok(self.publish(json))
    }

    /// Subscribe to receive raw string events (backward compatibility).
    ///
    /// Prefer `subscribe_typed()` for new code.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }

    /// Subscribe to receive typed event frames.
    pub fn subscribe_typed(&self) -> broadcast::Receiver<GatewayEventFrame> {
        self.typed_sender.subscribe()
    }

    /// Get the current number of active subscribers (string channel).
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
            typed_sender: self.typed_sender.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_publish_subscribe_str() {
        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(r#"{"event":"test"}"#);

        let received = rx.recv().await.unwrap();
        assert!(received.contains("test"));
    }

    #[tokio::test]
    async fn test_publish_subscribe_typed() {
        use super::super::events::GatewayEventFrame;

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe_typed();

        let frame = GatewayEventFrame::SessionUpdated {
            session_key: "test-session".to_string(),
        };
        bus.publish_frame(&frame).unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            GatewayEventFrame::SessionUpdated { session_key } => {
                assert_eq!(session_key, "test-session");
            }
            _ => panic!("expected SessionUpdated"),
        }
    }

    #[tokio::test]
    async fn test_publish_dual_channel() {
        use super::super::events::GatewayEventFrame;

        let bus = GatewayEventBus::new();
        let mut str_rx = bus.subscribe();
        let mut typed_rx = bus.subscribe_typed();

        let frame = GatewayEventFrame::ConfigChanged {
            section: None,
            value: serde_json::json!({}),
        };
        bus.publish_frame(&frame).unwrap();

        let str_received = str_rx.recv().await.unwrap();
        assert!(str_received.contains("config_changed"));

        let typed_received = typed_rx.recv().await.unwrap();
        match typed_received {
            GatewayEventFrame::ConfigChanged { .. } => {}
            _ => panic!("expected ConfigChanged"),
        }
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = GatewayEventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let count = bus.publish(r#"{"event":"multi"}"#);
        assert_eq!(count, 2);

        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[test]
    fn test_no_subscribers() {
        let bus = GatewayEventBus::new();
        let count = bus.publish("test");
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
        let filter =
            TopicFilter::with_patterns(vec!["agent.run.*".to_string(), "session.*".to_string()]);

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
}

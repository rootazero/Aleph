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

/// Topic on which `runtimes.install` progress events are published. The Panel
/// subscribes to this (`events.subscribe`) and renders a live install status
/// per runtime card.
pub const RUNTIME_INSTALL_PROGRESS_TOPIC: &str = "runtimes.install.progress";

/// Runtime install progress event.
///
/// Published during `runtimes.install` (wrapped in a [`TopicEvent`] on
/// [`RUNTIME_INSTALL_PROGRESS_TOPIC`]) to stream step-by-step progress to the
/// Panel UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInstallProgressEvent {
    /// Install step: capability name being installed (e.g. "fnm", "node", "playwright-cli")
    pub step: String,
    /// Step status: "started" | "done" | "failed"
    pub status: String,
    /// Error message (present when status == "failed")
    pub error: Option<String>,
    /// Raw stderr captured from the failing install command. Populated only
    /// when `status == "failed"` and upstream error carried stderr context.
    #[serde(default)]
    pub stderr: Option<String>,
    /// Milliseconds since Unix epoch
    pub timestamp: i64,
}

/// Gateway Event types
#[derive(Debug, Clone)]
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
    /// Snapshot of the `StateVersionTracker` at emit time. Populated only
    /// for events emitted right after a presence/health/config bump, via
    /// [`TopicEvent::with_state_version`]. Skipped from the wire when
    /// `None` so non-bump events keep their original envelope size.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state_version: Option<crate::gateway::state_version::StateVersion>,
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
            state_version: None,
        }
    }

    /// Attach a state-version snapshot to this event. Call right after a
    /// `state_versions.bump_*()` so subscribers can advance their cached
    /// version baseline without a separate round-trip.
    #[must_use]
    pub const fn with_state_version(
        mut self,
        version: crate::gateway::state_version::StateVersion,
    ) -> Self {
        self.state_version = Some(version);
        self
    }

    /// Convert to JSON-RPC notification format
    #[must_use]
    pub fn to_notification(&self) -> Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "event",
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
#[must_use]
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

/// A single equality predicate against a field inside the event's `data`
/// payload. `field` is a dot-separated path resolved one segment at a time,
/// so `"scope"`, `"device.role"`, or `"meta.tags.0"` all work.
///
/// `equals` is matched with `==` against the resolved [`serde_json::Value`].
/// Strings, numbers, booleans, and JSON null all work; nested objects compare
/// structurally (rarely useful — prefer narrowing the path).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldPredicate {
    /// Dot-separated path inside the event's `data` object.
    pub field: String,
    /// Required value at that path for the event to be delivered.
    pub equals: Value,
}

/// A subscription entry: a topic pattern plus an optional list of
/// field-equality predicates. When `where_clause` is empty the subscription
/// is topic-only (the pre-T3 behaviour). When non-empty, the event is
/// delivered only if every predicate resolves true against the event's
/// `data` field — useful for splitting a noisy fan-out topic like
/// `tools.changed` into per-`scope` channels without server-side knowledge
/// of subscriber intent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopicSubscription {
    /// Glob-style topic pattern (see [`topic_matches`]).
    pub pattern: String,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub where_clause: Vec<FieldPredicate>,
}

impl TopicSubscription {
    /// Build a topic-only subscription (no field predicates) — the same
    /// behaviour as the original `Vec<String>` patterns.
    pub fn pattern_only(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            where_clause: Vec::new(),
        }
    }
}

fn resolve_field<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = data;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(seg)?,
            Value::Array(items) => {
                let idx: usize = seg.parse().ok()?;
                items.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

fn where_clause_matches(predicates: &[FieldPredicate], data: Option<&Value>) -> bool {
    if predicates.is_empty() {
        return true;
    }
    let Some(data) = data else {
        return false; // predicates requested but no payload data → no match
    };
    predicates
        .iter()
        .all(|p| resolve_field(data, &p.field) == Some(&p.equals))
}

/// A subscription filter for topic-based events
#[derive(Debug, Clone)]
pub struct TopicFilter {
    subscriptions: Vec<TopicSubscription>,
}

impl TopicFilter {
    /// Create a filter that matches all events
    #[must_use]
    pub fn all() -> Self {
        Self {
            subscriptions: vec![TopicSubscription::pattern_only("*")],
        }
    }

    /// Create a filter with specific patterns (no field predicates).
    pub fn with_patterns(patterns: Vec<String>) -> Self {
        Self {
            subscriptions: patterns
                .into_iter()
                .map(TopicSubscription::pattern_only)
                .collect(),
        }
    }

    /// Create a filter with full subscription entries (patterns + optional
    /// `where_clause` predicates).
    #[must_use]
    pub const fn with_subscriptions(subscriptions: Vec<TopicSubscription>) -> Self {
        Self { subscriptions }
    }

    /// Check if a topic + payload satisfies any subscription in this filter.
    /// Pass `None` for `data` when no payload context is available; entries
    /// with field predicates will then be skipped (predicates can never match
    /// against missing data).
    #[must_use]
    pub fn matches(&self, topic: &str, data: Option<&Value>) -> bool {
        self.subscriptions.iter().any(|sub| {
            topic_matches(topic, &sub.pattern) && where_clause_matches(&sub.where_clause, data)
        })
    }

    /// Add a pattern-only subscription (no field predicates) to the filter.
    pub fn add_pattern(&mut self, pattern: impl Into<String>) {
        self.add_subscription(TopicSubscription::pattern_only(pattern));
    }

    /// Add a full subscription entry (pattern + optional predicates).
    /// Idempotent: skip if an exact-match (same pattern + same `where_clause`)
    /// is already present. Clients sometimes re-call `events.subscribe` from
    /// reconnect handlers / re-rendered Effects; without this dedup the
    /// pattern list grows unbounded and every matching event ends up
    /// dispatched N times to the same connection.
    pub fn add_subscription(&mut self, subscription: TopicSubscription) {
        if self.subscriptions.iter().any(|s| s == &subscription) {
            return;
        }
        self.subscriptions.push(subscription);
    }

    /// Remove every subscription whose pattern equals `pattern`. Returns true
    /// if at least one entry was removed.
    pub fn remove_pattern(&mut self, pattern: &str) -> bool {
        let initial_len = self.subscriptions.len();
        self.subscriptions.retain(|s| s.pattern != pattern);
        self.subscriptions.len() < initial_len
    }

    /// Get all patterns currently subscribed to (sans `where_clause`
    /// metadata) — sufficient for the `events.list` response shape.
    #[must_use]
    pub fn patterns(&self) -> Vec<String> {
        self.subscriptions
            .iter()
            .map(|s| s.pattern.clone())
            .collect()
    }

    /// Get the full subscription entries, including any `where_clause`
    /// predicates. Use this when callers need to round-trip the richer
    /// shape.
    #[must_use]
    pub fn subscriptions(&self) -> &[TopicSubscription] {
        &self.subscriptions
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
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        let (typed_sender, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        Self {
            sender,
            typed_sender,
        }
    }

    /// Create a new event bus with custom channel size
    #[must_use]
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
    /// For the string channel (consumed by WebSocket handler), the frame is
    /// wrapped in the wire format expected by the handler and frontend:
    ///
    /// - **Streaming events** (agent run lifecycle): wrapped as JSON-RPC
    ///   notification `{"method": "stream.<type>", "params": <frame>}`.
    ///   The handler extracts `method` for subscription matching (`stream.*`),
    ///   and the frontend converts `stream.X` → `run.X` for dispatch.
    ///
    /// - **Other events** (config, channel, etc.): wrapped as `TopicEvent`
    ///   `{"topic": "<topic_name>", "data": <frame>}`.
    ///   The handler wraps this into `{"method": "event", "params": {...}}`.
    pub fn publish_frame(&self, frame: &GatewayEventFrame) -> Result<usize, serde_json::Error> {
        // Truncate on a char boundary (frame Debug embeds user/model text which
        // is frequently multibyte UTF-8); byte-slicing would panic mid-char.
        let preview: String = format!("{frame:?}").chars().take(100).collect();
        debug!("Publishing typed event: {}", preview);
        let frame_value = serde_json::to_value(frame)?;
        let wire_json = if let Some(method) = frame.stream_method() {
            // Streaming events → JSON-RPC notification format
            serde_json::json!({
                "method": method,
                "params": frame_value,
            })
        } else {
            // Non-streaming events → TopicEvent format
            serde_json::json!({
                "topic": frame.topic_name(),
                "data": frame_value,
            })
        };
        let json = serde_json::to_string(&wire_json)?;
        let typed_count = self.typed_sender.send(frame.clone()).unwrap_or_else(|e| {
            // A `SendError` here means there are zero subscribers OR every
            // subscriber lagged behind the broadcast ring (capacity 1024 by
            // default). The old code flattened both into `0`, leaving
            // operators with no signal that frames were being silently lost.
            // Log at warn so the lag-behind case is observable; the no-
            // subscriber case is benign and stays at trace.
            let _lag = e.0;
            if self.typed_sender.receiver_count() == 0 {
                tracing::trace!("typed event has no subscribers; dropping");
            } else {
                tracing::warn!(
                    subscribers = self.typed_sender.receiver_count(),
                    "typed event dropped: subscribers fell behind broadcast ring; \
                     consider raising broadcast capacity or slowing publish rate"
                );
            }
            // Best-effort: still try to deliver via the string channel.
            0
        });
        let str_count = self.sender.send(json).unwrap_or_else(|e| {
            let lag = e.0;
            if self.sender.receiver_count() == 0 {
                tracing::trace!("string event has no subscribers; dropping");
            } else {
                tracing::warn!(
                    subscribers = self.sender.receiver_count(),
                    "string event dropped: subscribers fell behind broadcast ring"
                );
            }
            // Suppress unused warning on lag.
            let _ = lag;
            0
        });
        Ok(typed_count.max(str_count))
    }

    /// Publish a raw JSON string event to the string channel.
    ///
    /// For new code, prefer `publish_frame()` to get typed channel support.
    pub fn publish(&self, event: impl AsRef<str>) -> usize {
        let event_str = event.as_ref();
        let preview = if event_str.chars().count() > 100 {
            let truncated: String = event_str.chars().take(100).collect();
            format!("{truncated}...")
        } else {
            event_str.to_string()
        };
        debug!("Publishing event: {}", preview);
        self.sender.send(event_str.to_string()).unwrap_or_else(|e| {
            if self.sender.receiver_count() == 0 {
                tracing::trace!("event has no subscribers; dropping");
            } else {
                tracing::warn!(
                    subscribers = self.sender.receiver_count(),
                    "event dropped: subscribers fell behind broadcast ring"
                );
            }
            let _ = e;
            0
        })
    }

    /// Publish a typed event by serializing it to JSON.
    pub fn publish_json<T: serde::Serialize>(&self, event: &T) -> Result<usize, serde_json::Error> {
        let json = serde_json::to_string(event)?;
        Ok(self.publish(json))
    }

    pub fn publish_gateway_event(&self, event: &GatewayEvent) -> Result<usize, serde_json::Error> {
        match event {
            GatewayEvent::ConfigChanged(event) => {
                self.publish_frame(&GatewayEventFrame::ConfigChanged {
                    section: event.section.clone(),
                    value: event.value.clone(),
                })
            }
        }
    }

    /// Subscribe to receive raw string events (backward compatibility).
    ///
    /// Prefer `subscribe_typed()` for new code.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }

    /// Subscribe to receive typed event frames.
    #[must_use]
    pub fn subscribe_typed(&self) -> broadcast::Receiver<GatewayEventFrame> {
        self.typed_sender.subscribe()
    }

    /// Get the current number of active subscribers (string channel).
    #[must_use]
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
            origin_channel: Some("telegram".to_string()),
            origin_run_id: Some("run-7".to_string()),
        };
        bus.publish_frame(&frame).unwrap();

        let received = rx.recv().await.unwrap();
        match received {
            GatewayEventFrame::SessionUpdated {
                session_key,
                origin_channel,
                origin_run_id,
            } => {
                assert_eq!(session_key, "test-session");
                assert_eq!(origin_channel.as_deref(), Some("telegram"));
                assert_eq!(origin_run_id.as_deref(), Some("run-7"));
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

        assert!(filter.matches("agent.run.started", None));
        assert!(filter.matches("agent.run.completed", None));
        assert!(filter.matches("session.created", None));
        assert!(!filter.matches("config.updated", None));
    }

    #[test]
    fn test_field_filter_matches_when_predicate_satisfied() {
        let filter = TopicFilter::with_subscriptions(vec![TopicSubscription {
            pattern: "tools.changed".to_string(),
            where_clause: vec![FieldPredicate {
                field: "scope".to_string(),
                equals: Value::String("extension".to_string()),
            }],
        }]);

        let extension_data = serde_json::json!({"scope": "extension", "detail": {}});
        let mcp_data = serde_json::json!({"scope": "mcp", "detail": {}});

        assert!(filter.matches("tools.changed", Some(&extension_data)));
        assert!(!filter.matches("tools.changed", Some(&mcp_data)));
        // Predicate present but no data → must not match (can't verify).
        assert!(!filter.matches("tools.changed", None));
        // Topic mismatch wins even when data could satisfy the where clause.
        assert!(!filter.matches("session.created", Some(&extension_data)));
    }

    #[test]
    fn test_field_filter_dot_path_traverses_nested_objects() {
        let filter = TopicFilter::with_subscriptions(vec![TopicSubscription {
            pattern: "presence.changed".to_string(),
            where_clause: vec![FieldPredicate {
                field: "device.role".to_string(),
                equals: Value::String("operator".to_string()),
            }],
        }]);

        let operator = serde_json::json!({"device": {"role": "operator"}});
        let viewer = serde_json::json!({"device": {"role": "viewer"}});
        assert!(filter.matches("presence.changed", Some(&operator)));
        assert!(!filter.matches("presence.changed", Some(&viewer)));
    }

    #[test]
    fn test_topic_only_subscription_ignores_data() {
        let filter = TopicFilter::with_patterns(vec!["agent.run.*".to_string()]);
        let anything = serde_json::json!({"whatever": 1});
        assert!(filter.matches("agent.run.started", Some(&anything)));
        assert!(filter.matches("agent.run.started", None));
    }

    #[test]
    fn test_topic_filter_all() {
        let filter = TopicFilter::all();
        assert!(filter.matches("anything", None));
        assert!(filter.matches("any.nested.topic", None));
    }

    #[test]
    fn topic_event_with_state_version_roundtrips() {
        let snap = crate::gateway::state_version::StateVersion {
            presence: 7,
            health: 2,
            config: 0,
        };
        let ev = TopicEvent::new("presence.joined", serde_json::json!({})).with_state_version(snap);
        assert_eq!(ev.state_version, Some(snap));

        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("state_version"));

        let back: TopicEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state_version, Some(snap));
    }

    #[test]
    fn topic_event_without_version_omits_field() {
        let ev = TopicEvent::new("presence.joined", serde_json::json!({}));
        assert!(ev.state_version.is_none());

        // skip_serializing_if keeps the wire envelope unchanged for
        // non-bump events.
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("state_version"));

        // Absent field deserializes back to None via #[serde(default)].
        let back: TopicEvent = serde_json::from_str(&json).unwrap();
        assert!(back.state_version.is_none());
    }

    #[test]
    fn add_pattern_dedups_repeat_calls() {
        let mut f = TopicFilter::with_patterns(vec![]);
        f.add_pattern("stream.session_updated");
        f.add_pattern("stream.session_updated");
        f.add_pattern("stream.session_updated");
        assert_eq!(f.patterns(), vec!["stream.session_updated".to_string()]);
    }

    #[test]
    fn add_subscription_dedups_pattern_plus_where_clause() {
        let mut f = TopicFilter::with_patterns(vec![]);
        let sub_a = TopicSubscription {
            pattern: "tools.changed".into(),
            where_clause: vec![FieldPredicate {
                field: "scope".into(),
                equals: serde_json::json!("extension"),
            }],
        };
        let sub_b_same = sub_a.clone();
        let sub_c_diff_where = TopicSubscription {
            pattern: "tools.changed".into(),
            where_clause: vec![FieldPredicate {
                field: "scope".into(),
                equals: serde_json::json!("mcp"),
            }],
        };
        f.add_subscription(sub_a);
        f.add_subscription(sub_b_same); // dropped — exact dup
        f.add_subscription(sub_c_diff_where); // kept — different predicate
        assert_eq!(f.subscriptions().len(), 2);
    }
}

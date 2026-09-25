//! Event Subscription Handlers
//!
//! Handles subscribing and unsubscribing from event topics.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::gateway::event_bus::{TopicFilter, TopicSubscription};
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Tracks subscriptions per connection
pub struct SubscriptionManager {
    /// Map of connection ID to their topic filters
    subscriptions: RwLock<HashMap<String, TopicFilter>>,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscriptions: RwLock::new(HashMap::new()),
        }
    }

    /// Add patterns to a connection's filter
    pub async fn add_patterns(&self, conn_id: &str, patterns: Vec<String>) {
        let mut subs = self.subscriptions.write().await;
        let filter = subs
            .entry(conn_id.to_string())
            .or_insert_with(|| TopicFilter::with_patterns(vec![]));
        for pattern in patterns {
            filter.add_pattern(pattern);
        }
    }

    /// Add full subscription entries (pattern + optional `where_clause`
    /// field predicates) to a connection's filter.
    pub async fn add_subscriptions(&self, conn_id: &str, subscriptions: Vec<TopicSubscription>) {
        let mut subs = self.subscriptions.write().await;
        let filter = subs
            .entry(conn_id.to_string())
            .or_insert_with(|| TopicFilter::with_patterns(vec![]));
        for sub in subscriptions {
            filter.add_subscription(sub);
        }
    }

    /// Remove the entries of `patterns` carrying exactly the carve-out
    /// `except` from a connection's filter.
    pub async fn remove_patterns(
        &self,
        conn_id: &str,
        patterns: &[String],
        except: &[String],
    ) -> usize {
        let mut subs = self.subscriptions.write().await;
        if let Some(filter) = subs.get_mut(conn_id) {
            let mut removed = 0;
            for pattern in patterns {
                if filter.remove(pattern, except) {
                    removed += 1;
                }
            }
            return removed;
        }
        0
    }

    /// Remove a connection's subscriptions entirely
    pub async fn remove_connection(&self, conn_id: &str) {
        let mut subs = self.subscriptions.write().await;
        subs.remove(conn_id);
    }

    /// Check if a connection should receive an event with the given topic
    /// and optional payload `data`. Field-predicate filters consult `data`
    /// — pass `None` only when payload context isn't available (subscriptions
    /// with predicates will then be skipped).
    pub async fn should_receive(&self, conn_id: &str, topic: &str, data: Option<&Value>) -> bool {
        let subs = self.subscriptions.read().await;
        match subs.get(conn_id) {
            Some(filter) => filter.matches(topic, data),
            None => true, // No filter means receive all (default behavior)
        }
    }

    /// Get patterns for a connection
    pub async fn get_patterns(&self, conn_id: &str) -> Vec<String> {
        let subs = self.subscriptions.read().await;
        subs.get(conn_id).map(|f| f.patterns()).unwrap_or_default()
    }

    /// Get the full subscription entries (with `where_clause` predicates)
    /// for a connection.
    pub async fn get_subscriptions(&self, conn_id: &str) -> Vec<TopicSubscription> {
        let subs = self.subscriptions.read().await;
        subs.get(conn_id)
            .map(|f| f.subscriptions().to_vec())
            .unwrap_or_default()
    }
}

impl Default for SubscriptionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// A single topic selector in the `events.subscribe` payload. Accepts
/// either a plain pattern string (back-compat, no field filter) or a
/// structured object `{topic, where: [{field, equals}]}` for field-level
/// filtering (T3 follow-up).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TopicSelector {
    /// Topic-only subscription: just the pattern string.
    Pattern(String),
    /// Pattern + optional `where_clause` field predicates.
    Filtered {
        /// Topic glob pattern (see `topic_matches`).
        topic: String,
        /// Field-equality predicates. Empty list ≡ topic-only.
        #[serde(default, rename = "where")]
        where_clause: Vec<crate::gateway::event_bus::FieldPredicate>,
    },
}

impl TopicSelector {
    fn into_subscription(self) -> TopicSubscription {
        match self {
            Self::Pattern(p) => TopicSubscription::pattern_only(p),
            Self::Filtered {
                topic,
                where_clause,
            } => TopicSubscription {
                pattern: topic,
                where_clause,
                except: Vec::new(),
            },
        }
    }
}

/// Parameters for events.subscribe
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeParams {
    /// Topic patterns or `{topic, where: …}` filter objects to subscribe to.
    pub topics: Vec<TopicSelector>,
    /// Patterns carved out of every entry this call adds (the wire key lives
    /// in `aleph_protocol::TopicCarveOut`).
    #[serde(flatten)]
    pub carve_out: aleph_protocol::TopicCarveOut,
}

/// Parameters for events.unsubscribe: remove the entries of these patterns
/// that carry exactly this carve-out (none when `except` is absent). See
/// [`TopicFilter::remove`](crate::gateway::event_bus::TopicFilter::remove).
pub type UnsubscribeParams = aleph_protocol::TopicsRequest;

/// Result of subscription operations
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionResult {
    /// Current subscribed patterns
    pub subscribed: Vec<String>,
    /// Number of patterns added/removed
    pub changed: usize,
}

/// Handle "events.subscribe" request
///
/// Subscribes the connection to specified topic patterns or
/// `{topic, where: …}` filter objects. Plain strings stay backwards-
/// compatible with pre-T3 clients; the filtered form lets a subscriber
/// narrow a noisy topic (e.g. `tools.changed` with `scope=extension`).
pub async fn handle_subscribe(
    request: JsonRpcRequest,
    conn_id: &str,
    manager: Arc<SubscriptionManager>,
) -> JsonRpcResponse {
    let params: SubscribeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.topics.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Topics array cannot be empty");
    }

    let count = params.topics.len();
    let except = params.carve_out.except;
    let subs: Vec<TopicSubscription> = params
        .topics
        .into_iter()
        .map(|t| t.into_subscription().excepting(except.clone()))
        .collect();
    manager.add_subscriptions(conn_id, subs).await;
    let subscribed = manager.get_patterns(conn_id).await;

    info!(
        conn_id = %conn_id,
        patterns = ?subscribed,
        "Connection subscribed to topics"
    );

    JsonRpcResponse::success(
        request.id,
        json!(SubscriptionResult {
            subscribed,
            changed: count,
        }),
    )
}

/// Handle "events.unsubscribe" request
///
/// Unsubscribes the connection from specified topic patterns.
pub async fn handle_unsubscribe(
    request: JsonRpcRequest,
    conn_id: &str,
    manager: Arc<SubscriptionManager>,
) -> JsonRpcResponse {
    let params: UnsubscribeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let removed = manager
        .remove_patterns(conn_id, &params.topics, &params.carve_out.except)
        .await;
    let subscribed = manager.get_patterns(conn_id).await;

    debug!(
        conn_id = %conn_id,
        removed = removed,
        "Connection unsubscribed from topics"
    );

    JsonRpcResponse::success(
        request.id,
        json!(SubscriptionResult {
            subscribed,
            changed: removed,
        }),
    )
}

/// Handle "events.list" request
///
/// Returns the current subscriptions for the connection.
pub async fn handle_list(
    request: JsonRpcRequest,
    conn_id: &str,
    manager: Arc<SubscriptionManager>,
) -> JsonRpcResponse {
    let subscribed = manager.get_patterns(conn_id).await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "subscribed": subscribed,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscription_manager() {
        let manager = SubscriptionManager::new();

        // Add subscriptions
        manager
            .add_patterns(
                "conn1",
                vec!["agent.*".to_string(), "session.*".to_string()],
            )
            .await;

        // Check filtering
        assert!(
            manager
                .should_receive("conn1", "agent.run.started", None)
                .await
        );
        assert!(
            manager
                .should_receive("conn1", "session.created", None)
                .await
        );
        assert!(
            !manager
                .should_receive("conn1", "config.updated", None)
                .await
        );

        // Unknown connection receives all (default)
        assert!(manager.should_receive("unknown", "anything", None).await);
    }

    #[tokio::test]
    async fn test_remove_patterns() {
        let manager = SubscriptionManager::new();

        manager
            .add_patterns(
                "conn1",
                vec!["agent.*".to_string(), "session.*".to_string()],
            )
            .await;

        let removed = manager
            .remove_patterns("conn1", &["agent.*".to_string()], &[])
            .await;
        assert_eq!(removed, 1);

        assert!(!manager.should_receive("conn1", "agent.run", None).await);
        assert!(
            manager
                .should_receive("conn1", "session.created", None)
                .await
        );
    }

    #[tokio::test]
    async fn test_field_filter_round_trip_via_subscribe_request() {
        use crate::gateway::event_bus::FieldPredicate;
        let manager = Arc::new(SubscriptionManager::new());

        let request = JsonRpcRequest::new(
            "events.subscribe",
            Some(json!({
                "topics": [
                    "agent.run.*",
                    {"topic": "tools.changed", "where": [
                        {"field": "scope", "equals": "extension"}
                    ]}
                ]
            })),
            Some(json!(1)),
        );
        let response = handle_subscribe(request, "conn-field", manager.clone()).await;
        assert!(
            response.is_success(),
            "subscribe should accept the mixed-shape payload: {response:?}"
        );

        // Topic-only subscription still works for any payload.
        assert!(
            manager
                .should_receive("conn-field", "agent.run.started", None)
                .await
        );

        // Filtered subscription delivers only when the predicate is satisfied.
        let extension = json!({"scope": "extension"});
        let mcp = json!({"scope": "mcp"});
        assert!(
            manager
                .should_receive("conn-field", "tools.changed", Some(&extension))
                .await
        );
        assert!(
            !manager
                .should_receive("conn-field", "tools.changed", Some(&mcp))
                .await
        );
        // No payload → can't verify predicate → drop.
        assert!(
            !manager
                .should_receive("conn-field", "tools.changed", None)
                .await
        );

        // Confirm the get_subscriptions API round-trips the where clause.
        let subs = manager.get_subscriptions("conn-field").await;
        let filtered = subs.iter().find(|s| s.pattern == "tools.changed").unwrap();
        assert_eq!(filtered.where_clause.len(), 1);
        assert_eq!(
            filtered.where_clause[0],
            FieldPredicate {
                field: "scope".to_string(),
                equals: json!("extension"),
            }
        );
    }

    fn topics(topics: &[&str], except: &[&str]) -> Value {
        serde_json::to_value(aleph_protocol::TopicsRequest::new(
            topics.iter().map(|t| (*t).to_string()).collect(),
            except.iter().map(|e| (*e).to_string()).collect(),
        ))
        .expect("serialize")
    }

    async fn call(
        method: &str,
        params: Value,
        conn: &str,
        manager: &Arc<SubscriptionManager>,
    ) -> JsonRpcResponse {
        let request = JsonRpcRequest::new(method, Some(params), Some(json!(1)));
        let response = match method {
            "events.subscribe" => handle_subscribe(request, conn, manager.clone()).await,
            _ => handle_unsubscribe(request, conn, manager.clone()).await,
        };
        assert!(response.is_success(), "{method}: {response:?}");
        response
    }

    async fn gets_reasoning(manager: &SubscriptionManager, conn: &str) -> bool {
        manager
            .should_receive(conn, aleph_protocol::STREAM_REASONING_TOPIC, None)
            .await
    }

    /// R-G14: a client that never renders reasoning (the phone) takes
    /// `stream.*` minus `stream.reasoning`; the rest of the pattern still
    /// arrives.
    ///
    /// Mutation-checked: ignoring the carve-out in `TopicSubscription::admits`
    /// turns this red.
    #[tokio::test]
    async fn a_carve_out_withholds_only_the_named_topic() {
        let manager = Arc::new(SubscriptionManager::new());
        let reasoning = aleph_protocol::STREAM_REASONING_TOPIC;
        call(
            "events.subscribe",
            topics(&["stream.*"], &[reasoning]),
            "c",
            &manager,
        )
        .await;
        assert!(
            !gets_reasoning(&manager, "c").await,
            "the carved-out topic is withheld"
        );
        assert!(
            manager
                .should_receive("c", "stream.response_chunk", None)
                .await,
            "the rest of the pattern still arrives"
        );
    }

    /// §19: the Panel swaps its phone chat (`stream.*` minus reasoning) and
    /// its wide chat (plain `stream.*`) over ONE socket on a resize, and the
    /// subscribe of one and the unsubscribe of the other may reach the server
    /// in either order. The two are distinct entries — the carve-out is part
    /// of the key — so in both orders the wide chat ends up with reasoning and
    /// the phone without, and each unsubscribe removes only its own entry.
    ///
    /// Mutation-checked: letting one entry's carve-out narrow the whole
    /// connection, or ignoring `except` when removing, turns this red.
    #[tokio::test]
    async fn phone_and_wide_entries_coexist_and_leave_independently() {
        let reasoning = aleph_protocol::STREAM_REASONING_TOPIC;
        let phone = || topics(&["stream.*"], &[reasoning]);
        let wide = || topics(&["stream.*"], &[]);

        // Phone → wide: the wide subscribe lands before the phone's cleanup.
        let m = Arc::new(SubscriptionManager::new());
        call("events.subscribe", phone(), "c", &m).await;
        call("events.subscribe", wide(), "c", &m).await;
        assert!(
            gets_reasoning(&m, "c").await,
            "the wide entry is not narrowed by the phone's"
        );
        call("events.unsubscribe", phone(), "c", &m).await;
        assert!(
            gets_reasoning(&m, "c").await,
            "the phone's cleanup leaves the wide entry"
        );
        assert!(m.should_receive("c", "stream.response_chunk", None).await);

        // Wide → phone: the phone subscribe lands before the wide cleanup.
        let m = Arc::new(SubscriptionManager::new());
        call("events.subscribe", wide(), "c", &m).await;
        call("events.subscribe", phone(), "c", &m).await;
        call("events.unsubscribe", wide(), "c", &m).await;
        assert!(
            !gets_reasoning(&m, "c").await,
            "the wide cleanup removes only the plain entry"
        );
        assert!(
            m.should_receive("c", "stream.response_chunk", None).await,
            "the phone's entry survives the wide cleanup"
        );
    }

    /// The CLI twin's `**` minus reasoning must deliver exactly what the CLI
    /// got unsubscribed (`should_receive` = `None => true`) for everything but
    /// reasoning — a missing `RunComplete` would hang `ask`. Checked over every
    /// topic string `frame.rs` publishes, plus the empty topic
    /// `extract_topic_and_data` falls back to for an unrecognised shape. And an
    /// older server, which reads the request as plain `**`, delivers every one
    /// of them, reasoning included: `topic_matches` returns early for `**`.
    ///
    /// Nothing else reaching the socket goes through this filter at all: RPC
    /// responses, reverse-RPC frames and the overflow diagnostic are written
    /// straight to the socket (`server/handler.rs`), and the event forwarder is
    /// the one caller of `should_receive`.
    #[tokio::test]
    async fn the_cli_carve_out_delivers_everything_an_unsubscribed_socket_did_but_reasoning() {
        let frame_src = include_str!("../events/frame.rs");
        let mut names: Vec<String> = frame_src
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| {
                s.contains('.')
                    && s.chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c == '.')
            })
            .map(str::to_string)
            .collect();
        names.sort();
        names.dedup();
        assert!(
            names.iter().any(|t| t == "stream.run_complete")
                && names.iter().any(|t| t == "stream.ask_user")
                && names.len() >= 30,
            "the topic scrape stopped matching frame.rs: {names:?}"
        );
        names.push(String::new());
        names.push(aleph_protocol::STREAM_REASONING_TOPIC.to_string());

        let m = Arc::new(SubscriptionManager::new());
        let cli = topics(&["**"], &[aleph_protocol::STREAM_REASONING_TOPIC]);
        call("events.subscribe", cli, "cli", &m).await;
        let old = Arc::new(SubscriptionManager::new());
        call("events.subscribe", topics(&["**"], &[]), "cli", &old).await;
        for t in &names {
            let unsubscribed = m.should_receive("never-subscribed", t, None).await;
            assert!(unsubscribed);
            let expected = t != aleph_protocol::STREAM_REASONING_TOPIC;
            assert_eq!(m.should_receive("cli", t, None).await, expected, "{t:?}");
            assert!(
                old.should_receive("cli", t, None).await,
                "old server, {t:?}"
            );
        }
    }

    /// Version skew: the Panel can reach an older LAN server. The params as
    /// the new clients send them must parse under both older server shapes —
    /// the pre-T3 `Vec<String>` topic list and the T3 selector list — and
    /// there subscribe the plain topic, i.e. today's behaviour (reasoning
    /// delivered), never a rejected call that leaves the client streamless.
    #[test]
    fn an_older_server_reads_the_carve_out_as_a_plain_subscription() {
        #[derive(Deserialize)]
        struct PreT3 {
            topics: Vec<String>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        #[allow(dead_code)]
        enum T3Selector {
            Pattern(String),
            Filtered {
                topic: String,
                #[serde(default, rename = "where")]
                where_clause: Vec<Value>,
            },
        }
        #[derive(Deserialize)]
        struct T3 {
            topics: Vec<T3Selector>,
        }

        let new = topics(&["stream.*"], &[aleph_protocol::STREAM_REASONING_TOPIC]);
        let pre: PreT3 = serde_json::from_value(new.clone()).expect("pre-T3 server parses it");
        assert_eq!(pre.topics, vec!["stream.*".to_string()]);
        let t3: T3 = serde_json::from_value(new).expect("T3 server parses it");
        assert!(matches!(t3.topics.as_slice(), [T3Selector::Pattern(p)] if p == "stream.*"));
    }

    #[tokio::test]
    async fn test_handle_subscribe() {
        let manager = Arc::new(SubscriptionManager::new());

        let request = JsonRpcRequest::new(
            "events.subscribe",
            Some(json!({"topics": ["agent.*", "session.*"]})),
            Some(json!(1)),
        );

        let response = handle_subscribe(request, "test-conn", manager.clone()).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result.get("changed").unwrap().as_u64().unwrap(), 2);

        let subscribed = result.get("subscribed").unwrap().as_array().unwrap();
        assert_eq!(subscribed.len(), 2);
    }
}

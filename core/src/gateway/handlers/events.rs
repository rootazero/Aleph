//! Event Subscription Handlers
//!
//! Handles subscribing and unsubscribing from event topics.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::gateway::event_bus::TopicFilter;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Tracks subscriptions per connection
pub struct SubscriptionManager {
    /// Map of connection ID to their topic filters
    subscriptions: RwLock<HashMap<String, TopicFilter>>,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new() -> Self {
        Self {
            subscriptions: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a filter for a connection
    pub async fn get_filter(&self, conn_id: &str) -> Option<TopicFilter> {
        let subs = self.subscriptions.read().await;
        subs.get(conn_id).cloned()
    }

    /// Set the filter for a connection
    pub async fn set_filter(&self, conn_id: &str, filter: TopicFilter) {
        let mut subs = self.subscriptions.write().await;
        subs.insert(conn_id.to_string(), filter);
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

    /// Remove patterns from a connection's filter
    pub async fn remove_patterns(&self, conn_id: &str, patterns: &[String]) -> usize {
        let mut subs = self.subscriptions.write().await;
        if let Some(filter) = subs.get_mut(conn_id) {
            let mut removed = 0;
            for pattern in patterns {
                if filter.remove_pattern(pattern) {
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
    pub async fn should_receive(&self, conn_id: &str, topic: &str) -> bool {
        let subs = self.subscriptions.read().await;
        match subs.get(conn_id) {
            Some(filter) => filter.matches(topic),
            None => true, // No filter means receive all (default behavior)
        }
    }

    /// Get patterns for a connection
    pub async fn get_patterns(&self, conn_id: &str) -> Vec<String> {
        let subs = self.subscriptions.read().await;
        subs.get(conn_id)
            .map(|f| f.patterns().to_vec())
            .unwrap_or_default()
    }
}

impl Default for SubscriptionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for events.subscribe
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeParams {
    /// Topic patterns to subscribe to
    pub topics: Vec<String>,
}

/// Parameters for events.unsubscribe
#[derive(Debug, Clone, Deserialize)]
pub struct UnsubscribeParams {
    /// Topic patterns to unsubscribe from
    pub topics: Vec<String>,
}

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
/// Subscribes the connection to specified topic patterns.
pub async fn handle_subscribe(
    request: JsonRpcRequest,
    conn_id: &str,
    manager: Arc<SubscriptionManager>,
) -> JsonRpcResponse {
    let params: SubscribeParams = match &request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map.clone())) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    if params.topics.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Topics array cannot be empty");
    }

    let count = params.topics.len();
    manager.add_patterns(conn_id, params.topics).await;
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
    let params: UnsubscribeParams = match &request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map.clone())) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let removed = manager.remove_patterns(conn_id, &params.topics).await;
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
            .add_patterns("conn1", vec!["agent.*".to_string(), "session.*".to_string()])
            .await;

        // Check filtering
        assert!(manager.should_receive("conn1", "agent.run.started").await);
        assert!(manager.should_receive("conn1", "session.created").await);
        assert!(!manager.should_receive("conn1", "config.updated").await);

        // Unknown connection receives all (default)
        assert!(manager.should_receive("unknown", "anything").await);
    }

    #[tokio::test]
    async fn test_remove_patterns() {
        let manager = SubscriptionManager::new();

        manager
            .add_patterns("conn1", vec!["agent.*".to_string(), "session.*".to_string()])
            .await;

        let removed = manager
            .remove_patterns("conn1", &["agent.*".to_string()])
            .await;
        assert_eq!(removed, 1);

        assert!(!manager.should_receive("conn1", "agent.run").await);
        assert!(manager.should_receive("conn1", "session.created").await);
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

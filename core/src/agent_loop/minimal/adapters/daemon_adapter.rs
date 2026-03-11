//! Daemon event source adapters exposing the daemon/event system as MinimalTool.
//!
//! Defines a `DaemonBackend` trait for testability and two tools:
//! - `DaemonQueryTool` — query active system events and notifications
//! - `DaemonSubscribeTool` — subscribe to new event monitoring rules

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use super::super::tool::{MinimalTool, ToolResult};

// =============================================================================
// DaemonBackend trait
// =============================================================================

/// A single daemon/system event.
#[derive(Debug, Clone)]
pub struct DaemonEvent {
    pub event_type: String,
    pub description: String,
    pub timestamp: i64,
}

/// Abstraction over the daemon event source for testability.
#[async_trait]
pub trait DaemonBackend: Send + Sync {
    /// Query all currently active events.
    async fn query_active_events(&self) -> anyhow::Result<Vec<DaemonEvent>>;

    /// Subscribe to events matching the given rule pattern.
    /// Returns a subscription ID.
    async fn subscribe(&self, rule: &str) -> anyhow::Result<String>;
}

// =============================================================================
// DaemonQueryTool
// =============================================================================

/// Tool that queries active daemon events via a `DaemonBackend`.
pub struct DaemonQueryTool<D: DaemonBackend> {
    backend: Arc<D>,
}

impl<D: DaemonBackend> DaemonQueryTool<D> {
    pub fn new(backend: Arc<D>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<D: DaemonBackend + 'static> MinimalTool for DaemonQueryTool<D> {
    fn name(&self) -> &str {
        "daemon_query"
    }

    fn description(&self) -> &str {
        "Query active system events and notifications"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        match self.backend.query_active_events().await {
            Ok(events) => {
                let events_json: Vec<Value> = events
                    .iter()
                    .map(|e| {
                        json!({
                            "type": e.event_type,
                            "description": e.description,
                            "timestamp": e.timestamp,
                        })
                    })
                    .collect();
                ToolResult::Success {
                    output: json!({ "events": events_json }),
                }
            }
            Err(e) => ToolResult::Error {
                error: format!("daemon query failed: {e}"),
                retryable: true,
            },
        }
    }
}

// =============================================================================
// DaemonSubscribeTool
// =============================================================================

/// Tool that subscribes to daemon event patterns via a `DaemonBackend`.
pub struct DaemonSubscribeTool<D: DaemonBackend> {
    backend: Arc<D>,
}

impl<D: DaemonBackend> DaemonSubscribeTool<D> {
    pub fn new(backend: Arc<D>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl<D: DaemonBackend + 'static> MinimalTool for DaemonSubscribeTool<D> {
    fn name(&self) -> &str {
        "daemon_subscribe"
    }

    fn description(&self) -> &str {
        "Subscribe to a new type of system event monitoring"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "rule": {
                    "type": "string",
                    "description": "Event pattern to watch for, e.g. 'file:~/project/**' or 'cron:0 8 * * *'"
                }
            },
            "required": ["rule"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let rule = match input.get("rule").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => {
                return ToolResult::Error {
                    error: "missing required parameter: rule".into(),
                    retryable: false,
                };
            }
        };

        match self.backend.subscribe(rule).await {
            Ok(subscription_id) => ToolResult::Success {
                output: json!({ "subscription_id": subscription_id }),
            },
            Err(e) => ToolResult::Error {
                error: format!("daemon subscribe failed: {e}"),
                retryable: true,
            },
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::Mutex;

    /// In-memory fake daemon backend for testing.
    struct FakeDaemon {
        events: Vec<DaemonEvent>,
        next_sub_id: Mutex<u64>,
    }

    impl FakeDaemon {
        fn new(events: Vec<DaemonEvent>) -> Self {
            Self {
                events,
                next_sub_id: Mutex::new(1),
            }
        }
    }

    #[async_trait]
    impl DaemonBackend for FakeDaemon {
        async fn query_active_events(&self) -> anyhow::Result<Vec<DaemonEvent>> {
            Ok(self.events.clone())
        }

        async fn subscribe(&self, _rule: &str) -> anyhow::Result<String> {
            let mut next = self.next_sub_id.lock().await;
            let id = format!("sub-{}", *next);
            *next += 1;
            Ok(id)
        }
    }

    #[tokio::test]
    async fn test_query_returns_events() {
        let backend = Arc::new(FakeDaemon::new(vec![
            DaemonEvent {
                event_type: "file_change".into(),
                description: "File modified: src/main.rs".into(),
                timestamp: 1700000000,
            },
            DaemonEvent {
                event_type: "cron".into(),
                description: "Scheduled backup completed".into(),
                timestamp: 1700000060,
            },
        ]));
        let tool = DaemonQueryTool::new(Arc::clone(&backend));

        let result = tool.execute(json!({})).await;

        match result {
            ToolResult::Success { output } => {
                let events = output["events"].as_array().unwrap();
                assert_eq!(events.len(), 2);
                assert_eq!(events[0]["type"], "file_change");
                assert_eq!(events[0]["description"], "File modified: src/main.rs");
                assert_eq!(events[0]["timestamp"], 1700000000);
                assert_eq!(events[1]["type"], "cron");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_query_empty_events() {
        let backend = Arc::new(FakeDaemon::new(vec![]));
        let tool = DaemonQueryTool::new(Arc::clone(&backend));

        let result = tool.execute(json!({})).await;

        match result {
            ToolResult::Success { output } => {
                let events = output["events"].as_array().unwrap();
                assert!(events.is_empty());
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_returns_subscription_id() {
        let backend = Arc::new(FakeDaemon::new(vec![]));
        let tool = DaemonSubscribeTool::new(Arc::clone(&backend));

        let result = tool
            .execute(json!({ "rule": "file:~/project/**" }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["subscription_id"], "sub-1");
            }
            ToolResult::Error { error, .. } => panic!("expected success, got: {error}"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_missing_rule() {
        let backend = Arc::new(FakeDaemon::new(vec![]));
        let tool = DaemonSubscribeTool::new(Arc::clone(&backend));

        let result = tool.execute(json!({})).await;

        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("missing required parameter: rule"));
                assert!(!retryable);
            }
            ToolResult::Success { .. } => panic!("expected error"),
        }
    }
}

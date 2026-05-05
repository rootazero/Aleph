//! TimeoutLayer — per-tool timeout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::timeout;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct TimeoutLayer {
    inner: Arc<dyn ToolService>,
    default_timeout: Duration,
    per_tool_override: HashMap<String, Duration>,
}

impl TimeoutLayer {
    pub fn new(
        inner: Arc<dyn ToolService>,
        default_timeout: Duration,
        per_tool_override: HashMap<String, Duration>,
    ) -> Self {
        Self {
            inner,
            default_timeout,
            per_tool_override,
        }
    }

    fn timeout_for(&self, name: &str) -> Duration {
        self.per_tool_override
            .get(name)
            .copied()
            .unwrap_or(self.default_timeout)
    }
}

#[async_trait]
impl ToolService for TimeoutLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let t = self.timeout_for(name);
        match timeout(t, self.inner.execute(name, input)).await {
            Ok(result) => result,
            Err(_) => Err(ToolError::Timeout {
                name: name.to_string(),
                elapsed_ms: t.as_millis() as u64,
            }),
        }
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        self.inner.dispatcher_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::service::ToolService;
    use async_trait::async_trait;

    struct SlowInner {
        delay: Duration,
    }

    #[async_trait]
    impl ToolService for SlowInner {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolOutput {
                value: serde_json::json!("done"),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _n: &str) -> Option<ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fast_tool_succeeds() {
        let inner = Arc::new(SlowInner {
            delay: Duration::from_millis(10),
        });
        let layer = TimeoutLayer::new(inner, Duration::from_secs(1), HashMap::new());
        let out = layer.execute("x", serde_json::json!({})).await.unwrap();
        assert_eq!(out.value, serde_json::json!("done"));
    }

    #[tokio::test(start_paused = true)]
    async fn slow_tool_times_out() {
        let inner = Arc::new(SlowInner {
            delay: Duration::from_secs(10),
        });
        let layer = TimeoutLayer::new(inner, Duration::from_millis(100), HashMap::new());
        let err = layer.execute("x", serde_json::json!({})).await.unwrap_err();
        assert!(matches!(
            err,
            ToolError::Timeout {
                elapsed_ms: 100,
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn per_tool_override_wins() {
        let inner = Arc::new(SlowInner {
            delay: Duration::from_secs(10),
        });
        let mut overrides = HashMap::new();
        overrides.insert("slow".to_string(), Duration::from_millis(50));
        let layer = TimeoutLayer::new(inner, Duration::from_secs(100), overrides);
        let err = layer
            .execute("slow", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Timeout { elapsed_ms: 50, .. }));
    }
}

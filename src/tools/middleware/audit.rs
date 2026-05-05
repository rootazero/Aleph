//! ExecAuditLayer — tracing + latency for tool execution.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct ExecAuditLayer {
    inner: Arc<dyn ToolService>,
}

impl ExecAuditLayer {
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolService for ExecAuditLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        tracing::info!(target: "tool.call", tool = %name, phase = "start");
        match self.inner.execute(name, input).await {
            Ok(mut out) => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                tracing::info!(
                    target: "tool.call",
                    tool = %name,
                    phase = "ok",
                    elapsed_ms,
                );
                out.metadata.latency_ms = elapsed_ms;
                Ok(out)
            }
            Err(e) => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                tracing::warn!(
                    target: "tool.call",
                    tool = %name,
                    phase = "err",
                    elapsed_ms,
                    error = %e,
                );
                Err(e)
            }
        }
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        std::sync::Arc::from([])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOp;

    #[async_trait]
    impl ToolService for NoOp {
        async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    #[tokio::test]
    async fn stamps_latency_on_success() {
        let inner = Arc::new(NoOp);
        let layer = ExecAuditLayer::new(inner);
        let out = layer.execute("x", serde_json::json!({})).await.unwrap();
        assert!(out.metadata.latency_ms < 1_000_000);
    }
}

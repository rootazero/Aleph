//! TimeoutLayer — per-tool timeout. Task 8 fills in.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct TimeoutLayer {
    inner: Arc<dyn ToolService>,
}

impl TimeoutLayer {
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolService for TimeoutLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        self.inner.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }
}

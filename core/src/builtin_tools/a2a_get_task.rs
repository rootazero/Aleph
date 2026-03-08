//! A2A Get Task Tool
//!
//! Retrieves the status of a task from a remote A2A agent.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::port::AgentResolver;
use crate::a2a::service::CardRegistry;
use crate::error::AlephError;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2AGetTaskArgs {
    /// Agent ID that owns the task
    pub agent_id: String,
    /// Task ID to retrieve
    pub task_id: String,
    /// Number of history messages to include (optional)
    #[serde(default)]
    pub history_length: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2AGetTaskResult {
    pub task_id: String,
    pub state: String,
    pub message: Option<String>,
    pub artifacts_count: usize,
    pub history_count: usize,
}

#[derive(Clone)]
pub struct A2AGetTaskTool {
    registry: Arc<CardRegistry>,
    client_pool: Arc<A2AClientPool>,
}

impl A2AGetTaskTool {
    pub fn new(registry: Arc<CardRegistry>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            registry,
            client_pool,
        }
    }
}

#[async_trait::async_trait]
impl AlephTool for A2AGetTaskTool {
    const NAME: &'static str = "a2a_get_task";
    const DESCRIPTION: &'static str =
        "Get the status of a task from a remote A2A agent.";

    type Args = A2AGetTaskArgs;
    type Output = A2AGetTaskResult;

    async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
        // 1. Resolve agent by ID
        let agent = self
            .registry
            .resolve_by_id(&args.agent_id)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to resolve agent: {}", e)))?
            .ok_or_else(|| {
                AlephError::tool(format!("Agent not found: {}", args.agent_id))
            })?;

        // 2. Get client from pool
        let client = self
            .client_pool
            .get_or_create(&agent)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to get client: {}", e)))?;

        // 3. Fetch task status
        let task = client
            .get_task(&args.task_id, args.history_length)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to get task: {}", e)))?;

        // 4. Extract response text
        let message = task
            .status
            .message
            .as_ref()
            .map(|m| m.text_content())
            .filter(|t| !t.is_empty());

        Ok(A2AGetTaskResult {
            task_id: task.id,
            state: format!("{:?}", task.status.state),
            message,
            artifacts_count: task.artifacts.len(),
            history_count: task.history.len(),
        })
    }
}

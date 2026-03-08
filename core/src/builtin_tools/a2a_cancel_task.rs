//! A2A Cancel Task Tool
//!
//! Cancels a running task on a remote A2A agent.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::port::AgentResolver;
use crate::a2a::service::CardRegistry;
use crate::error::AlephError;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2ACancelTaskArgs {
    /// Agent ID that owns the task
    pub agent_id: String,
    /// Task ID to cancel
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2ACancelTaskResult {
    pub task_id: String,
    pub state: String,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct A2ACancelTaskTool {
    registry: Arc<CardRegistry>,
    client_pool: Arc<A2AClientPool>,
}

impl A2ACancelTaskTool {
    pub fn new(registry: Arc<CardRegistry>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            registry,
            client_pool,
        }
    }
}

#[async_trait::async_trait]
impl AlephTool for A2ACancelTaskTool {
    const NAME: &'static str = "a2a_cancel_task";
    const DESCRIPTION: &'static str =
        "Cancel a running task on a remote A2A agent.";

    type Args = A2ACancelTaskArgs;
    type Output = A2ACancelTaskResult;

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

        // 3. Cancel the task
        let task = client
            .cancel_task(&args.task_id)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to cancel task: {}", e)))?;

        // 4. Extract status message
        let message = task
            .status
            .message
            .as_ref()
            .map(|m| m.text_content())
            .filter(|t| !t.is_empty());

        Ok(A2ACancelTaskResult {
            task_id: task.id,
            state: format!("{:?}", task.status.state),
            message,
        })
    }
}

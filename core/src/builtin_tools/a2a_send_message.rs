//! A2A Send Message Tool
//!
//! Sends a text message to a remote A2A agent and optionally waits for completion.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::a2a::adapter::client::A2AClientPool;
use crate::a2a::domain::{A2AMessage, A2ARole};
use crate::a2a::port::AgentResolver;
use crate::a2a::service::CardRegistry;
use crate::error::AlephError;
use crate::tools::AlephTool;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct A2ASendMessageArgs {
    /// Agent ID to send message to
    pub agent_id: String,
    /// Message text to send
    pub message: String,
    /// Whether to wait for completion (default: true)
    #[serde(default = "default_true")]
    pub wait: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2ASendMessageResult {
    pub task_id: String,
    pub state: String,
    pub response: Option<String>,
}

#[derive(Clone)]
pub struct A2ASendMessageTool {
    registry: Arc<CardRegistry>,
    client_pool: Arc<A2AClientPool>,
}

impl A2ASendMessageTool {
    pub fn new(registry: Arc<CardRegistry>, client_pool: Arc<A2AClientPool>) -> Self {
        Self {
            registry,
            client_pool,
        }
    }
}

#[async_trait::async_trait]
impl AlephTool for A2ASendMessageTool {
    const NAME: &'static str = "a2a_send_message";
    const DESCRIPTION: &'static str =
        "Send a message to a remote A2A agent and get its response.";

    type Args = A2ASendMessageArgs;
    type Output = A2ASendMessageResult;

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

        // 2. Get or create client from pool
        let client = self
            .client_pool
            .get_or_create(&agent)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to get client: {}", e)))?;

        // 3. Build message
        let message = A2AMessage::text(A2ARole::User, &args.message);
        let task_id = uuid::Uuid::new_v4().to_string();

        // 4. Send message
        let task = client
            .send_message(&task_id, &message, None)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to send message: {}", e)))?;

        // 5. Extract text response from task status message
        let response = task
            .status
            .message
            .as_ref()
            .map(|m| m.text_content())
            .filter(|t| !t.is_empty());

        Ok(A2ASendMessageResult {
            task_id: task.id,
            state: format!("{:?}", task.status.state),
            response,
        })
    }
}

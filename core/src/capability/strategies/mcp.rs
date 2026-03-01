//! MCP (Model Context Protocol) capability strategy.
//!
//! Provides AI models with access to MCP tools via builtin services
//! and external server connections.

use crate::capability::strategy::CapabilityStrategy;
use crate::config::McpConfig;
use crate::error::Result;
use crate::mcp::McpClient;
use crate::payload::{AgentPayload, Capability};
use async_trait::async_trait;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tracing::{debug, info};

/// MCP capability strategy
///
/// This strategy integrates with MCP servers to provide
/// additional tools and data sources to AI models.
///
/// It populates the payload with available MCP tools that can be
/// included in the system prompt for the AI to understand what
/// capabilities are available.
pub struct McpStrategy {
    /// MCP client for tool access
    mcp_client: Option<Arc<McpClient>>,
    /// MCP configuration
    mcp_config: Option<Arc<McpConfig>>,
}

impl McpStrategy {
    /// Create a new MCP strategy
    pub fn new() -> Self {
        Self {
            mcp_client: None,
            mcp_config: None,
        }
    }

    /// Create a new MCP strategy with client
    pub fn with_client(client: Option<Arc<McpClient>>, config: Option<Arc<McpConfig>>) -> Self {
        Self {
            mcp_client: client,
            mcp_config: config,
        }
    }

    /// Set the MCP client
    pub fn set_client(&mut self, client: Arc<McpClient>) {
        self.mcp_client = Some(client);
    }
}

impl Default for McpStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CapabilityStrategy for McpStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Mcp
    }

    fn priority(&self) -> u32 {
        2 // MCP executes after Memory and Search
    }

    fn is_available(&self) -> bool {
        // Check if MCP is enabled and client is available
        if let Some(config) = &self.mcp_config {
            if !config.enabled {
                return false;
            }
        }
        self.mcp_client.is_some()
    }

    fn validate_config(&self) -> Result<()> {
        // MCP config validation is done at client initialization
        // Here we just check consistency
        if let Some(config) = &self.mcp_config {
            if !config.enabled {
                // MCP is disabled, nothing to validate
                return Ok(());
            }
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        if !self.is_available() {
            return Ok(false);
        }

        // Check if client has any tools available
        if let Some(client) = &self.mcp_client {
            let tools = client.list_tools().await;
            debug!(
                tool_count = tools.len(),
                "MCP health check: tools available"
            );
            // MCP is healthy if we have at least some tools
            // (could be 0 if all servers are down, but client is still running)
            return Ok(true);
        }

        Ok(false)
    }

    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert("capability".to_string(), "Mcp".to_string());
        info.insert("name".to_string(), "mcp".to_string());
        info.insert("priority".to_string(), "2".to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info.insert(
            "has_client".to_string(),
            self.mcp_client.is_some().to_string(),
        );
        info.insert(
            "has_config".to_string(),
            self.mcp_config.is_some().to_string(),
        );
        if let Some(config) = &self.mcp_config {
            info.insert("enabled".to_string(), config.enabled.to_string());
        }
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Check if client is available
        let Some(client) = &self.mcp_client else {
            debug!("MCP capability requested but no client configured");
            return Ok(payload);
        };

        // Check if MCP is enabled
        if let Some(config) = &self.mcp_config {
            if !config.enabled {
                debug!("MCP capability disabled in config");
                return Ok(payload);
            }
        }

        info!("Executing MCP capability - listing available tools");

        // Get available tools from the MCP client
        let tools = client.list_tools().await;

        if tools.is_empty() {
            debug!("No MCP tools available");
            return Ok(payload);
        }

        info!(tool_count = tools.len(), "MCP tools available");

        // Convert tools to mcp_resources format
        // Format: tool_name -> { description, input_schema, requires_confirmation }
        let mut resources: HashMap<String, serde_json::Value> = HashMap::new();

        for tool in tools {
            let tool_info = serde_json::json!({
                "description": tool.description,
                "input_schema": tool.input_schema,
                "requires_confirmation": tool.requires_confirmation,
            });
            resources.insert(tool.name, tool_info);
        }

        payload.context.mcp_resources = Some(resources);

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_strategy_not_available() {
        let strategy = McpStrategy::new();
        assert!(!strategy.is_available());
    }

    #[test]
    fn test_mcp_strategy_with_disabled_config() {
        let config = McpConfig {
            enabled: false,
            ..McpConfig::default()
        };

        let strategy = McpStrategy::with_client(None, Some(Arc::new(config)));
        assert!(!strategy.is_available());
    }

    #[tokio::test]
    async fn test_mcp_strategy_execute_noop() {
        use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

        let strategy = McpStrategy::new();

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Mcp],
                ContextFormat::Markdown,
            )
            .user_input("Test".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.mcp_resources.is_none());
    }
}

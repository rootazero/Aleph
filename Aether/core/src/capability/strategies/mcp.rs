//! MCP (Model Context Protocol) capability strategy.
//!
//! This is a placeholder strategy for future MCP integration.
//! MCP allows AI models to access external tools and data sources.

use crate::capability::strategy::CapabilityStrategy;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use async_trait::async_trait;
use tracing::warn;

/// MCP capability strategy (placeholder)
///
/// This strategy will integrate with MCP servers to provide
/// additional tools and data sources to AI models.
///
/// Currently not implemented - reserved for future use.
pub struct McpStrategy {
    // Future fields:
    // mcp_client: Option<Arc<McpClient>>,
    // mcp_config: Option<Arc<McpConfig>>,
}

impl McpStrategy {
    /// Create a new MCP strategy
    pub fn new() -> Self {
        Self {}
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
        // MCP is not yet implemented
        false
    }

    async fn execute(&self, payload: AgentPayload) -> Result<AgentPayload> {
        warn!("MCP capability not implemented yet (reserved for future)");
        // Future: Call MCP client and populate payload.context.mcp_resources
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

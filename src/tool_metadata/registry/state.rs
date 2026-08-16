//! Tool State Management
//!
//! Methods for managing tool state and performing bulk operations.

use tracing::debug;

use super::types::ToolStorage;

/// State management functionality for `ToolCatalog`
pub struct ToolState {
    tools: ToolStorage,
}

impl ToolState {
    /// Create a new state manager with the given storage
    pub const fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// Clear all registered tools
    pub async fn clear(&self) {
        let mut tools = self.tools.write().await;
        tools.clear();
        debug!("Cleared all tools from registry");
    }

    /// Remove tools from a specific MCP server
    ///
    /// Used when restarting or removing a single MCP server without
    /// affecting other servers or tool sources.
    ///
    /// # Arguments
    ///
    /// * `server_name` - The MCP server name to remove tools for
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_mcp_server(&self, server_name: &str) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|_, tool| match &tool.source {
            super::super::types::ToolSource::Mcp { server } => server != server_name,
            _ => true,
        });

        let removed = initial_count - tools.len();
        debug!(
            server = server_name,
            removed = removed,
            "Removed MCP server tools"
        );
        removed
    }
}

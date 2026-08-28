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

    /// Set the active flag on a single tool by canonical name.
    ///
    /// Returns true if a tool with that name was found and its `is_active`
    /// value actually changed. The state write is scoped to the same write
    /// lock as `register_with_conflict_resolution` so an activate/deactivate
    /// cannot race with a registration that re-inserts the same tool — the
    /// later mutation wins, which matches the read-then-mutate contract of
    /// every sibling method.
    pub async fn set_active(&self, name: &str, active: bool) -> bool {
        let mut tools = self.tools.write().await;
        let mut changed = false;
        for (_id, tool) in tools.iter_mut() {
            if tool.name == name && tool.is_active != active {
                tool.is_active = active;
                changed = true;
            }
        }
        if changed {
            debug!(name = name, active = active, "Toggled tool active flag");
        }
        changed
    }
}

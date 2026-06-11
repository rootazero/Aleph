//! Tool State Management
//!
//! Methods for managing tool state and performing bulk operations.

use std::collections::HashMap;
use tracing::{debug, info};

use super::super::types::{ToolSourceType, UnifiedTool};
use super::types::ToolStorage;

/// State management functionality for ToolCatalog
pub struct ToolState {
    tools: ToolStorage,
}

impl ToolState {
    /// Create a new state manager with the given storage
    pub const fn new(tools: ToolStorage) -> Self {
        Self { tools }
    }

    /// Set tool active state
    ///
    /// # Arguments
    ///
    /// * `id` - Tool ID
    /// * `active` - Whether the tool should be active
    ///
    /// # Returns
    ///
    /// `true` if tool was found and updated, `false` otherwise
    pub async fn set_tool_active(&self, id: &str, active: bool) -> bool {
        let mut tools = self.tools.write().await;
        if let Some(tool) = tools.get_mut(id) {
            tool.is_active = active;
            debug!("Tool '{}' active state set to {}", id, active);
            true
        } else {
            false
        }
    }

    /// Clear all registered tools
    pub async fn clear(&self) {
        let mut tools = self.tools.write().await;
        tools.clear();
        debug!("Cleared all tools from registry");
    }

    /// Atomic refresh - build new HashMap and replace in one operation
    ///
    /// This method prevents the race condition where `clear()` and `register()`
    /// have a brief window of empty tool list. Instead, we build a completely
    /// new HashMap with all tools, then atomically replace the old one.
    ///
    /// # Arguments
    ///
    /// * `new_tools` - Vector of tools to register (replaces all existing)
    ///
    /// # Thread Safety
    ///
    /// This uses a single write lock operation, so UI will never see
    /// an empty or partially populated tool list during refresh.
    pub async fn refresh_atomic(&self, new_tools: Vec<UnifiedTool>) {
        let new_map: HashMap<String, UnifiedTool> =
            new_tools.into_iter().map(|t| (t.id.clone(), t)).collect();

        let count = new_map.len();

        // Single write lock operation - atomic replacement
        let mut tools = self.tools.write().await;
        *tools = new_map;
        // Lock released here - UI immediately sees new tools, no empty window

        info!("Tool registry atomically refreshed: {} tools", count);
    }

    /// Remove all tools of a specific source type
    ///
    /// This enables incremental updates - only refresh the affected source
    /// instead of clearing and re-registering everything.
    ///
    /// # Arguments
    ///
    /// * `source_type` - The source type to remove (Skill, Mcp, Custom, etc.)
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_by_source_type(&self, source_type: ToolSourceType) -> usize {
        let mut tools = self.tools.write().await;
        let initial_count = tools.len();

        tools.retain(|_, tool| ToolSourceType::from(&tool.source) != source_type);

        let removed = initial_count - tools.len();
        debug!(
            source_type = ?source_type,
            removed = removed,
            "Removed tools by source type"
        );
        removed
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

    /// Remove all skill tools
    ///
    /// Used when refreshing skills without affecting other tool sources.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_skills(&self) -> usize {
        self.remove_by_source_type(ToolSourceType::Skill).await
    }

    /// Remove all custom commands
    ///
    /// Used when updating routing rules without affecting other tool sources.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_custom_commands(&self) -> usize {
        self.remove_by_source_type(ToolSourceType::Custom).await
    }

    /// Remove all MCP tools (from all servers)
    ///
    /// Used when refreshing all MCP servers.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_all_mcp_tools(&self) -> usize {
        self.remove_by_source_type(ToolSourceType::Mcp).await
    }

    /// Remove all native tools
    ///
    /// Used when refreshing native tool configuration.
    ///
    /// # Returns
    ///
    /// Number of tools removed
    pub async fn remove_native_tools(&self) -> usize {
        self.remove_by_source_type(ToolSourceType::Native).await
    }
}

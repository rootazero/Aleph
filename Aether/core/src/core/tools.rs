//! Tool registry and dispatcher operations for AetherCore
//!
//! This module contains all tool registry methods:
//! - Tool listing and searching
//! - Tool registry refresh
//! - Command completion
//! - Async confirmation handling

use super::AetherCore;
use crate::error::Result;
use std::sync::Arc;
use tracing::{info, warn};

impl AetherCore {
    // ========================================================================
    // Tool Registry Methods (implement-dispatcher-layer)
    // ========================================================================

    /// Refresh the unified tool registry
    ///
    /// This method aggregates tools from all sources:
    /// - Native capabilities (Search, Video)
    /// - System tools (fs, git, shell, sys)
    /// - External MCP servers
    /// - Installed skills
    /// - Custom commands from config rules
    ///
    /// Call this method when:
    /// - Application starts (after initialization)
    /// - Config file changes (hot-reload callback)
    /// - MCP servers connect/disconnect
    /// - Skills are installed/removed
    pub fn refresh_tool_registry(&self) {
        let registry = Arc::clone(&self.tool_registry);
        let config = self.lock_config().clone();
        let mcp_client = self.mcp_client.clone();
        let event_handler = Arc::clone(&self.event_handler);

        // Run refresh in tokio runtime
        self.runtime.spawn(async move {
            // 0. Clear existing tools
            registry.clear().await;

            // 1. Register builtin commands (single source of truth)
            registry.register_builtin_tools().await;

            // 2. Register native tools (capabilities)
            registry.register_native_tools().await;

            // 2. Register system tools (from MCP client)
            if let Some(client) = &mcp_client {
                let tools = client.list_builtin_tools();
                let mcp_tool_infos: Vec<crate::mcp::McpToolInfo> = tools
                    .into_iter()
                    .map(|tool| {
                        let service_name = tool
                            .name
                            .split(':')
                            .next()
                            .unwrap_or("unknown")
                            .to_string();
                        crate::mcp::McpToolInfo {
                            name: tool.name,
                            description: tool.description,
                            requires_confirmation: tool.requires_confirmation,
                            service_name,
                        }
                    })
                    .collect();

                // Group by service name
                let mut by_service: std::collections::HashMap<String, Vec<crate::mcp::McpToolInfo>> =
                    std::collections::HashMap::new();
                for tool in mcp_tool_infos {
                    by_service
                        .entry(tool.service_name.clone())
                        .or_default()
                        .push(tool);
                }

                for (service_name, tools) in by_service {
                    registry
                        .register_mcp_tools(&tools, &service_name, true)
                        .await;
                }
            }

            // 3. Register skills
            if let Ok(skills) = crate::initialization::list_installed_skills() {
                registry.register_skills(&skills).await;
            }

            // 4. Register custom commands from routing rules
            registry.register_custom_commands(&config.rules).await;

            let tool_count = registry.active_count().await;
            info!("Tool registry refreshed: {} tools", tool_count);

            // Notify Swift that tools have changed
            event_handler.on_tools_changed(tool_count as u32);
        });
    }

    /// Get all registered tools from the unified registry
    ///
    /// Returns a list of all active tools for UI display or
    /// prompt generation.
    pub fn list_unified_tools(&self) -> Vec<crate::dispatcher::UnifiedTool> {
        self.runtime
            .block_on(async { self.tool_registry.list_all().await })
    }

    /// Search tools by name or description
    ///
    /// # Arguments
    /// * `query` - Search query string
    ///
    /// # Returns
    /// * `Vec<UnifiedTool>` - Matching tools sorted by relevance
    pub fn search_unified_tools(&self, query: &str) -> Vec<crate::dispatcher::UnifiedTool> {
        self.runtime
            .block_on(async { self.tool_registry.search(query).await })
    }

    /// Get tool registry prompt block for L3 routing
    ///
    /// Returns a markdown-formatted list of all active tools
    /// suitable for injection into the router LLM system prompt.
    pub fn get_tool_prompt_block(&self) -> String {
        self.runtime
            .block_on(async { self.tool_registry.to_prompt_block().await })
    }

    // ========================================================================
    // Dispatcher Layer FFI Methods (UniFFI exports)
    // ========================================================================

    /// List all available tools from unified registry
    ///
    /// Returns tools from all sources: Native, MCP, Skills, Custom.
    /// This is the UniFFI-exposed method that returns FFI-safe types.
    pub fn list_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.list_unified_tools()
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List builtin tools only (for Settings UI)
    ///
    /// Returns the 5 system builtin commands (/search, /mcp, /skill, /video, /chat)
    /// sorted by sort_order. This is the single source of truth for preset rules.
    pub fn list_builtin_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_builtin_tools().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List root-level commands for command completion
    ///
    /// Returns builtin commands + custom commands (but not nested MCP/Skill tools),
    /// sorted by sort_order then alphabetically.
    pub fn list_root_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_root_commands().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List tools filtered by source type
    ///
    /// # Arguments
    /// * `source_type` - Filter by this source type
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - Matching tools
    pub fn list_tools_by_source(
        &self,
        source_type: crate::dispatcher::ToolSourceType,
    ) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.list_unified_tools()
            .into_iter()
            .filter(|tool| crate::dispatcher::ToolSourceType::from(&tool.source) == source_type)
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// Search tools by name or description
    ///
    /// # Arguments
    /// * `query` - Search query string
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - Matching tools sorted by relevance
    pub fn search_tools(&self, query: String) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.search_unified_tools(&query)
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// Refresh tool registry (reload from all sources)
    ///
    /// This method triggers a refresh of the unified tool registry,
    /// re-aggregating tools from all sources.
    pub fn refresh_tools(&self) -> Result<()> {
        self.refresh_tool_registry();
        Ok(())
    }

    // ========================================================================
    // ASYNC CONFIRMATION METHODS (async-confirmation-flow)
    // ========================================================================

    /// Confirm or cancel a pending confirmation
    ///
    /// Called by Swift when user makes a decision on pending confirmation.
    /// Returns true if confirmation was found and processed, false if not found/expired.
    pub fn confirm_action(
        &self,
        confirmation_id: String,
        decision: crate::dispatcher::UserConfirmationDecision,
    ) -> Result<bool> {
        use crate::dispatcher::ConfirmationState;

        let state = self
            .async_confirmation
            .resume_with_decision(&confirmation_id, decision);

        match state {
            ConfirmationState::Confirmed {
                tool,
                parameters: _,
                routing_layer,
                confidence,
            } => {
                info!(
                    tool = %tool.name,
                    confidence = confidence,
                    layer = ?routing_layer,
                    "Confirmation confirmed, executing tool"
                );
                // TODO: Execute the tool asynchronously
                // For now, just log and return success
                Ok(true)
            }
            ConfirmationState::Cancelled { reason } => {
                info!(reason = %reason, "Confirmation cancelled");
                Ok(true)
            }
            ConfirmationState::TimedOut { confirmation_id } => {
                warn!(id = %confirmation_id, "Confirmation timed out");
                self.event_handler.on_confirmation_expired(confirmation_id);
                Ok(false)
            }
            ConfirmationState::NotRequired | ConfirmationState::Pending(_) => {
                // Should not happen in resume_with_decision
                Ok(false)
            }
        }
    }

    /// Cancel a pending confirmation by ID
    ///
    /// Returns true if confirmation was found and cancelled.
    pub fn cancel_confirmation(&self, confirmation_id: String) -> bool {
        self.async_confirmation.cancel(&confirmation_id)
    }

    /// Get pending confirmation by ID (if still valid)
    pub fn get_pending_confirmation(
        &self,
        confirmation_id: String,
    ) -> Option<crate::dispatcher::PendingConfirmationInfo> {
        self.async_confirmation
            .get_pending(&confirmation_id)
            .map(|p| p.to_ffi())
    }

    /// Get count of pending confirmations
    pub fn get_pending_confirmation_count(&self) -> u32 {
        self.async_confirmation.pending_count() as u32
    }

    /// Cleanup expired confirmations
    ///
    /// Returns count of cleaned confirmations.
    pub fn cleanup_expired_confirmations(&self) -> u32 {
        self.async_confirmation.cleanup_expired().len() as u32
    }

    // ========================================================================
    // COMMAND COMPLETION METHODS (add-command-completion-system)
    // ========================================================================

    /// Get all root-level commands for command completion UI
    ///
    /// Returns commands parsed from config.toml routing rules with ^/ prefix.
    /// Commands are sorted alphabetically by key.
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of root commands with key, description, icon, hint, type
    pub fn get_root_commands(&self) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config.general.language.as_deref().unwrap_or("en");
        let mut registry = crate::command::CommandRegistry::from_config(&config, language);

        // Inject installed skills as /skill subcommands
        if let Ok(skills) = crate::list_installed_skills() {
            registry.inject_skills(&skills);
        }

        registry.get_root_commands()
    }

    /// Get children of a namespace command
    ///
    /// For namespace commands like /mcp, returns the list of child commands.
    /// Currently returns empty for most namespaces (MCP integration reserved for future).
    ///
    /// # Arguments
    /// * `parent_key` - The key of the parent namespace (e.g., "mcp")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of child commands
    pub fn get_command_children(&self, parent_key: String) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config.general.language.as_deref().unwrap_or("en");
        let mut registry = crate::command::CommandRegistry::from_config(&config, language);

        // Inject installed skills as /skill subcommands
        if let Ok(skills) = crate::list_installed_skills() {
            registry.inject_skills(&skills);
        }

        registry.get_children(&parent_key)
    }

    /// Filter commands by key prefix (case-insensitive)
    ///
    /// Used for autocomplete as user types. Returns commands whose keys
    /// start with the given prefix.
    ///
    /// # Arguments
    /// * `prefix` - The prefix to filter by (e.g., "se" matches "search", "settings")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - Filtered list of matching commands
    pub fn filter_commands(&self, prefix: String) -> Vec<crate::command::CommandNode> {
        let commands = self.get_root_commands();
        crate::command::CommandRegistry::filter_by_prefix(&commands, &prefix)
    }

    // ========================================================================
    // TOOL REGISTRY-BASED COMMAND COMPLETION (unify-tool-registry)
    // ========================================================================

    /// Get subtools for a namespace command from ToolRegistry
    ///
    /// For commands like /mcp and /skill that have dynamic subtools,
    /// this queries the ToolRegistry to get the list of available subtools.
    ///
    /// # Arguments
    /// * `parent_key` - The key of the parent command (e.g., "mcp", "skill")
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - List of subtools, or empty if none
    pub fn get_subtools_from_registry(&self, parent_key: String) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        use crate::dispatcher::ToolSourceType;

        match parent_key.as_str() {
            "mcp" => {
                // Return all MCP tools
                self.list_tools_by_source(ToolSourceType::Mcp)
            }
            "skill" => {
                // Return all skill tools
                self.list_tools_by_source(ToolSourceType::Skill)
            }
            _ => Vec::new(),
        }
    }

    /// Convert UnifiedToolInfo to CommandNode for UI compatibility
    ///
    /// This allows the command completion UI to use ToolRegistry data
    /// while maintaining backward compatibility with CommandNode.
    pub fn tool_to_command_node(tool: &crate::dispatcher::UnifiedToolInfo) -> crate::command::CommandNode {
        use crate::command::{CommandNode, CommandType};

        let node_type = if tool.has_subtools {
            CommandType::Namespace
        } else {
            CommandType::Action
        };

        let mut node = CommandNode::new(
            tool.name.clone(),
            tool.description.clone(),
            node_type,
        );

        if let Some(icon) = &tool.icon {
            node = node.with_icon(icon.clone());
        }

        // Use source_id for tracking
        node = node.with_source_id(tool.id.clone());

        // Set has_children based on has_subtools
        node.has_children = tool.has_subtools;

        node
    }

    /// Get command completions from ToolRegistry
    ///
    /// Returns all root-level commands as CommandNode for UI display.
    /// This is an alternative to get_root_commands() that uses ToolRegistry as source.
    pub fn get_root_commands_from_registry(&self) -> Vec<crate::command::CommandNode> {
        self.list_root_tools()
            .iter()
            .map(Self::tool_to_command_node)
            .collect()
    }

    /// Get subcommand completions from ToolRegistry
    ///
    /// Returns subcommands for a namespace command as CommandNode for UI display.
    /// This is an alternative to get_command_children() that uses ToolRegistry as source.
    pub fn get_subcommands_from_registry(&self, parent_key: String) -> Vec<crate::command::CommandNode> {
        self.get_subtools_from_registry(parent_key)
            .iter()
            .map(Self::tool_to_command_node)
            .collect()
    }
}

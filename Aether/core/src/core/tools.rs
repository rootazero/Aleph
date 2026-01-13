//! Tool registry and dispatcher operations for AetherCore
//!
//! This module contains all tool registry methods:
//! - Tool listing and searching
//! - Tool registry refresh
//! - Command completion
//! - Async confirmation handling
//! - Native tool execution (AgentTool)

use super::AetherCore;
use crate::error::Result;
use crate::tools::{
    create_clipboard_tools, create_filesystem_tools, create_git_tools, create_screen_tools,
    create_shell_tools, create_system_tools, create_web_tools, FilesystemConfig, GitConfig,
    ScreenConfig, ShellConfig, ToolResult, WebFetchConfig,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

impl AetherCore {
    // ========================================================================
    // Tool Registry Methods (implement-dispatcher-layer)
    // ========================================================================

    /// Refresh the unified tool registry
    ///
    /// This method aggregates tools from all sources:
    /// - Native AgentTools (filesystem, git, shell, system, clipboard, screen)
    /// - System tools (legacy MCP-style, from MCP client)
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
        let native_registry = Arc::clone(&self.native_tool_registry);
        let config = self.lock_config().clone();
        let mcp_client = self.mcp_client.clone();
        let event_handler = Arc::clone(&self.event_handler);
        let intent_pipeline = self.intent_pipeline.clone();

        // Run refresh synchronously using block_on to ensure tools are available
        // immediately after AetherCore::new() returns. This is safe because
        // AetherCore initialization runs on a background thread.
        self.runtime.block_on(async move {
            // 0. Clear existing tools
            registry.clear().await;
            native_registry.clear().await;

            // 1. Register builtin commands (single source of truth)
            registry.register_builtin_tools().await;

            // 2. Register native AgentTools for execution
            // These are the new native function calling tools
            let native_tools = Self::create_native_agent_tools();
            let native_tool_count = native_registry.register_all(native_tools.clone()).await;
            debug!("Registered {} native AgentTools for execution", native_tool_count);

            // 2b. Also register native tools in the dispatcher registry for UI display
            for (service_name, tools) in Self::group_native_tools_by_service(&native_tools) {
                registry.register_agent_tools(&tools, &service_name).await;
            }

            // 3. External MCP server tools will be registered when servers connect
            // Native tools are now handled exclusively via AgentTool infrastructure above
            let _ = mcp_client; // Suppress unused warning

            // 4. Register skills
            if let Ok(skills) = crate::initialization::list_installed_skills() {
                registry.register_skills(&skills).await;
            }

            // 5. Register custom commands from routing rules
            registry.register_custom_commands(&config.rules).await;

            let tool_count = registry.active_count().await;
            info!("Tool registry refreshed: {} tools ({} native)", tool_count, native_tool_count);

            // 6. Update intent pipeline with the new tool list
            // This enables L3 LLM routing to be aware of available tools
            if let Some(ref pipeline) = intent_pipeline {
                let tools = registry.list_all().await;
                pipeline.update_tools(tools).await;
                debug!("Intent pipeline updated with {} tools", tool_count);
            }

            // Notify Swift that tools have changed
            event_handler.on_tools_changed(tool_count as u32);
        });
    }

    /// Create all native AgentTool instances
    ///
    /// Returns a flat list of all native tools with default configurations.
    /// These tools can be executed via `execute_native_tool()`.
    fn create_native_agent_tools() -> Vec<Arc<dyn crate::tools::AgentTool>> {
        let mut all_tools: Vec<Arc<dyn crate::tools::AgentTool>> = Vec::new();

        // Filesystem tools - allow access to home directory by default
        let fs_config = FilesystemConfig::with_home_dir();
        all_tools.extend(create_filesystem_tools(fs_config));

        // Git tools - allow all repositories by default (empty list = all allowed)
        let git_config = GitConfig::default();
        all_tools.extend(create_git_tools(git_config));

        // Shell tools - disabled by default for security
        // Users must explicitly configure allowed commands
        let shell_config = ShellConfig::default(); // Disabled by default
        all_tools.extend(create_shell_tools(shell_config));

        // System info tools
        all_tools.extend(create_system_tools());

        // Clipboard tools (returns tuple, extract just the tools)
        let (clipboard_tools, _ctx) = create_clipboard_tools();
        all_tools.extend(clipboard_tools);

        // Screen capture tools
        let screen_config = ScreenConfig::default();
        all_tools.extend(create_screen_tools(screen_config));

        // Web fetch tools (for fetching and extracting web page content)
        let web_config = WebFetchConfig::default();
        all_tools.extend(create_web_tools(web_config));

        all_tools
    }

    /// Group native tools by service name for dispatcher registration
    fn group_native_tools_by_service(
        tools: &[Arc<dyn crate::tools::AgentTool>],
    ) -> Vec<(String, Vec<Arc<dyn crate::tools::AgentTool>>)> {
        use std::collections::HashMap;

        let mut groups: HashMap<String, Vec<Arc<dyn crate::tools::AgentTool>>> = HashMap::new();

        for tool in tools {
            let service_name = match tool.category() {
                crate::tools::ToolCategory::Builtin => "builtin",
                crate::tools::ToolCategory::Native => "native",
                crate::tools::ToolCategory::Skills => "skills",
                crate::tools::ToolCategory::Mcp => "mcp",
                crate::tools::ToolCategory::Custom => "custom",
            };

            groups
                .entry(service_name.to_string())
                .or_default()
                .push(Arc::clone(tool));
        }

        groups.into_iter().collect()
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

    /// List builtin tools only
    ///
    /// Returns the 3 system builtin commands (/search, /youtube, /chat)
    /// sorted by sort_order.
    pub fn list_builtin_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_builtin_tools().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List preset tools for Settings UI (Flat Namespace Mode)
    ///
    /// Returns all non-Custom tools: Builtin + MCP + Skill + Native
    /// These are the "preset" tools displayed in Settings > Routing.
    pub fn list_preset_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_preset_tools().await })
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
    #[deprecated(note = "Not used by Swift layer, may be removed in future")]
    pub fn refresh_tools(&self) -> Result<()> {
        self.refresh_tool_registry();
        Ok(())
    }

    // ========================================================================
    // NATIVE TOOL EXECUTION (native-function-calling)
    // ========================================================================

    /// Execute a native tool by name with JSON arguments
    ///
    /// This method executes a tool registered in the NativeToolRegistry.
    /// Use this for executing AgentTool implementations with typed parameters.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name (e.g., "file_read", "git_status")
    /// * `args` - JSON string containing tool parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ToolResult)` - Execution result with success/error status
    /// * `Err(AetherError::ToolNotFound)` - Tool not registered
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = core.execute_native_tool(
    ///     "file_read".to_string(),
    ///     r#"{"path": "/tmp/test.txt"}"#.to_string()
    /// )?;
    /// ```
    pub fn execute_native_tool(&self, name: String, args: String) -> Result<ToolResult> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.execute(&name, &args).await
        })
    }

    /// Check if a native tool requires confirmation before execution
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name to check
    ///
    /// # Returns
    ///
    /// * `Some(true)` - Tool requires confirmation
    /// * `Some(false)` - Tool does not require confirmation
    /// * `None` - Tool not found
    pub fn native_tool_requires_confirmation(&self, name: String) -> Option<bool> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.requires_confirmation(&name).await
        })
    }

    /// Get all native tool definitions for LLM prompt generation
    ///
    /// Returns definitions sorted by category then name.
    /// These can be converted to OpenAI/Anthropic tool format.
    pub fn get_native_tool_definitions(&self) -> Vec<crate::tools::ToolDefinition> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.get_definitions().await
        })
    }

    /// Get native tools in OpenAI function calling format
    ///
    /// Returns a JSON array suitable for the OpenAI API `tools` parameter.
    pub fn get_native_tools_openai(&self) -> Vec<serde_json::Value> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.to_openai_tools().await
        })
    }

    /// Get native tools in Anthropic tool format
    ///
    /// Returns a JSON array suitable for the Anthropic API `tools` parameter.
    pub fn get_native_tools_anthropic(&self) -> Vec<serde_json::Value> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.to_anthropic_tools().await
        })
    }

    /// Get count of registered native tools
    pub fn native_tool_count(&self) -> u32 {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.count().await as u32
        })
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
        let registry = crate::command::CommandRegistry::from_config(&config, language);

        // NOTE: Flat namespace mode - skills are registered directly in ToolRegistry
        // No longer inject /skill namespace here

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
        let registry = crate::command::CommandRegistry::from_config(&config, language);

        // NOTE: Flat namespace mode - no namespace subcommands
        // /mcp and /skill are no longer supported as namespaces

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

    /// Convert UnifiedToolInfo to CommandNode for UI compatibility (Flat Namespace Mode)
    ///
    /// This allows the command completion UI to use ToolRegistry data.
    /// In flat namespace mode:
    /// - All tools are root-level commands (has_children is always false)
    /// - source_type indicates tool origin for badge display
    pub fn tool_to_command_node(tool: &crate::dispatcher::UnifiedToolInfo) -> crate::command::CommandNode {
        use crate::command::CommandNode;

        // In flat namespace mode, use new_with_source for proper source_type
        let mut node = CommandNode::new_with_source(
            tool.name.clone(),
            tool.description.clone(),
            tool.source_type,
        );

        if let Some(icon) = &tool.icon {
            node = node.with_icon(icon.clone());
        }

        // Use id for source_id tracking
        node = node.with_source_id(tool.id.clone());

        // In flat namespace mode, has_children is always false
        // (MCP subtools are registered as separate root commands)
        node.has_children = false;

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

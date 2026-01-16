//! Tool management methods for AetherCore
//!
//! This module contains tool-related methods: list_tools, add_mcp_tool, remove_tool, etc.

use super::{AetherCore, AetherFfiError, ToolInfoFFI};
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    /// List available tools
    ///
    /// Returns a list of all tools registered in the ToolServer.
    /// This includes built-in tools and any dynamically added MCP tools.
    pub fn list_tools(&self) -> Vec<ToolInfoFFI> {
        let tools = self.registered_tools.read().unwrap();
        tools.iter().map(|name| {
            let (description, source) = match name.as_str() {
                "search" => ("Search the internet".to_string(), "builtin".to_string()),
                "web_fetch" => ("Fetch web page content".to_string(), "builtin".to_string()),
                "youtube" => ("Extract YouTube video transcripts".to_string(), "builtin".to_string()),
                name if name.contains(':') => {
                    // MCP tool format: "server:tool_name"
                    let server = name.split(':').next().unwrap_or("mcp");
                    (format!("MCP tool from {}", server), format!("mcp:{}", server))
                }
                _ => ("Dynamic tool".to_string(), "dynamic".to_string()),
            };
            ToolInfoFFI {
                name: name.clone(),
                description,
                source,
            }
        }).collect()
    }

    /// Add an MCP tool dynamically (hot-reload)
    ///
    /// This method allows adding MCP tools at runtime when a new MCP server
    /// connects. The tool will be immediately available for all subsequent
    /// `process()` calls.
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool (should include server prefix, e.g., "server:tool")
    /// * `description` - Human-readable description
    /// * `parameters_schema` - JSON Schema string for tool parameters
    ///
    /// # Example
    /// ```rust,ignore
    /// core.add_mcp_tool(
    ///     "filesystem:read_file",
    ///     "Read contents of a file",
    ///     r#"{"type":"object","properties":{"path":{"type":"string"}}}"#
    /// );
    /// ```
    pub fn add_mcp_tool(
        &self,
        tool_name: String,
        description: String,
        parameters_schema: String,
    ) -> Result<(), AetherFfiError> {
        use crate::mcp::McpTool;
        use crate::rig_tools::McpToolWrapper;

        info!(tool_name = %tool_name, "Adding MCP tool dynamically");

        // Parse the JSON schema
        let schema: serde_json::Value = serde_json::from_str(&parameters_schema)
            .map_err(|e| AetherFfiError::Tool(format!("Invalid parameters schema: {}", e)))?;

        // Create McpTool definition
        let mcp_tool = McpTool {
            name: tool_name.clone(),
            description,
            input_schema: schema,
            requires_confirmation: false,
        };

        // Extract server name from tool name (format: "server:tool")
        let server_name = tool_name
            .split(':')
            .next()
            .unwrap_or("unknown")
            .to_string();

        // Note: We need an MCP client to execute the tool. For now, we create a placeholder.
        // In a full implementation, this should receive the actual McpClient.
        let placeholder_client = Arc::new(crate::mcp::McpClient::new());
        let wrapper = McpToolWrapper::new(mcp_tool, placeholder_client, server_name);

        // Add to tool server (async operation)
        let handle = self.tool_server_handle.clone();
        let registered_tools = Arc::clone(&self.registered_tools);
        let tool_name_clone = tool_name.clone();

        self.runtime.block_on(async move {
            handle.add_tool(wrapper).await
                .map_err(|e| AetherFfiError::Tool(format!("Failed to add tool: {}", e)))?;

            // Track the tool
            let mut tools = registered_tools.write().unwrap();
            if !tools.contains(&tool_name_clone) {
                tools.push(tool_name_clone.clone());
            }

            info!(tool_name = %tool_name_clone, "MCP tool added successfully");
            Ok(())
        })
    }

    /// Remove a tool dynamically (hot-reload)
    ///
    /// Removes a previously added tool from the ToolServer.
    /// The tool will no longer be available for subsequent `process()` calls.
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool to remove
    pub fn remove_tool(&self, tool_name: String) -> Result<(), AetherFfiError> {
        info!(tool_name = %tool_name, "Removing tool dynamically");

        let handle = self.tool_server_handle.clone();
        let registered_tools = Arc::clone(&self.registered_tools);
        let tool_name_clone = tool_name.clone();

        self.runtime.block_on(async move {
            handle.remove_tool(&tool_name_clone).await
                .map_err(|e| AetherFfiError::Tool(format!("Failed to remove tool: {}", e)))?;

            // Update tracking
            let mut tools = registered_tools.write().unwrap();
            tools.retain(|t| t != &tool_name_clone);

            info!(tool_name = %tool_name_clone, "Tool removed successfully");
            Ok(())
        })
    }

    /// Check if a tool is registered
    pub fn has_tool(&self, tool_name: String) -> bool {
        self.registered_tools.read().unwrap().contains(&tool_name)
    }

    /// Get the number of registered tools
    pub fn tool_count(&self) -> u32 {
        self.registered_tools.read().unwrap().len() as u32
    }

    /// List builtin tools only
    pub fn list_builtin_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        // Return static builtin tools
        vec![
            crate::dispatcher::UnifiedToolInfo {
                id: "builtin:search".to_string(),
                name: "search".to_string(),
                display_name: "Search".to_string(),
                description: "Search the internet".to_string(),
                source_type: crate::dispatcher::ToolSourceType::Builtin,
                source_id: None,
                parameters_schema: None,
                is_active: true,
                requires_confirmation: false,
                safety_level: "Read Only".to_string(),
                service_name: None,
                icon: Some("magnifyingglass".to_string()),
                usage: Some("/search <query>".to_string()),
                localization_key: Some("tool.search".to_string()),
                is_builtin: true,
                sort_order: 10,
                has_subtools: false,
            },
            crate::dispatcher::UnifiedToolInfo {
                id: "builtin:webfetch".to_string(),
                name: "webfetch".to_string(),
                display_name: "Web Fetch".to_string(),
                description: "Fetch web page content".to_string(),
                source_type: crate::dispatcher::ToolSourceType::Builtin,
                source_id: None,
                parameters_schema: None,
                is_active: true,
                requires_confirmation: false,
                safety_level: "Read Only".to_string(),
                service_name: None,
                icon: Some("globe".to_string()),
                usage: Some("/webfetch <url>".to_string()),
                localization_key: Some("tool.webfetch".to_string()),
                is_builtin: true,
                sort_order: 20,
                has_subtools: false,
            },
            crate::dispatcher::UnifiedToolInfo {
                id: "builtin:youtube".to_string(),
                name: "youtube".to_string(),
                display_name: "YouTube".to_string(),
                description: "Extract YouTube video transcripts".to_string(),
                source_type: crate::dispatcher::ToolSourceType::Builtin,
                source_id: None,
                parameters_schema: None,
                is_active: true,
                requires_confirmation: false,
                safety_level: "Read Only".to_string(),
                service_name: None,
                icon: Some("play.rectangle".to_string()),
                usage: Some("/youtube <video_url>".to_string()),
                localization_key: Some("tool.youtube".to_string()),
                is_builtin: true,
                sort_order: 30,
                has_subtools: false,
            },
        ]
    }

    /// Get root commands from the tool registry for command completion
    ///
    /// Returns all root-level commands as CommandNode for UI display.
    pub fn get_root_commands_from_registry(&self) -> Vec<crate::command::CommandNode> {
        // Convert builtin tools to CommandNode format
        self.list_builtin_tools()
            .iter()
            .map(|tool| crate::command::CommandNode {
                key: tool.name.clone(),
                description: tool.description.clone(),
                icon: tool.icon.clone().unwrap_or_else(|| "command".to_string()),
                hint: tool.usage.clone(),
                node_type: crate::command::CommandType::Action,
                has_children: tool.has_subtools,
                source_id: tool.source_id.clone(),
                source_type: tool.source_type,
            })
            .collect()
    }
}

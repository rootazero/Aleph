//! MCP Tool Bridge
//!
//! Bridges external MCP server tools to the AgentTool interface.
//! This allows external MCP tools to be used seamlessly with the native function
//! calling infrastructure.
//!
//! Note: Native tools (fs, git, shell, etc.) are implemented directly via the
//! `AgentTool` trait in the `tools` module. This bridge is only for external
//! MCP servers.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     McpToolBridge                                │
//! │  (Implements AgentTool trait for external MCP tools)            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  tool_def: McpTool      │  Metadata from MCP server             │
//! │  client: Arc<McpClient> │  Reference for tool execution         │
//! │  server_name: String    │  External server name                 │
//! └─────────────────────────────────────────────────────────────────┘
//!                                   │
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                       McpClient                                  │
//! │  call_tool(name, args) → McpToolResult                          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::mcp::{McpClient, McpToolBridge};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create MCP client
//! let client = Arc::new(McpClient::new());
//!
//! // Create bridges for all external MCP tools
//! let bridges = McpToolBridge::from_client(client.clone()).await;
//!
//! // Register in native tool registry
//! let registry = NativeToolRegistry::new();
//! for bridge in bridges {
//!     registry.register(Arc::new(bridge)).await;
//! }
//! ```

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::mcp::client::McpClient;
use crate::mcp::types::McpTool;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};


/// MCP Tool Bridge
///
/// Bridges an external MCP tool to the AgentTool interface.
/// This allows external MCP tools to be used with the native function calling system.
pub struct McpToolBridge {
    /// Tool definition from MCP
    tool_def: McpTool,
    /// MCP client for tool execution
    client: Arc<McpClient>,
    /// External server name
    server_name: String,
}

impl McpToolBridge {
    /// Create a new MCP tool bridge
    ///
    /// # Arguments
    ///
    /// * `tool_def` - The MCP tool definition
    /// * `client` - Arc reference to the MCP client
    /// * `server_name` - The external server name
    pub fn new(tool_def: McpTool, client: Arc<McpClient>, server_name: String) -> Self {
        Self {
            tool_def,
            client,
            server_name,
        }
    }

    /// Create bridges for all tools from an MCP client
    ///
    /// Creates `McpToolBridge` instances for all registered external MCP server tools.
    pub async fn from_client(client: Arc<McpClient>) -> Vec<Self> {
        let mut bridges = Vec::new();

        let all_tools = client.list_tools().await;
        for tool in all_tools {
            // External tools have format "server_name:tool_name"
            let server_name = tool
                .name
                .split(':')
                .next()
                .unwrap_or("unknown")
                .to_string();

            bridges.push(Self::new(tool, Arc::clone(&client), server_name));
        }

        bridges
    }

    /// Get the server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Convert McpTool to ToolDefinition
    fn to_tool_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_def.name.clone(),
            description: self.tool_def.description.clone(),
            parameters: self.tool_def.input_schema.clone(),
            requires_confirmation: self.tool_def.requires_confirmation,
            category: ToolCategory::Mcp,
        }
    }

    /// Convert McpToolResult to ToolResult
    fn convert_result(mcp_result: crate::mcp::types::McpToolResult) -> ToolResult {
        if mcp_result.success {
            // Extract content as string
            let content = match &mcp_result.content {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            };

            // If content is an object/array, include as data
            if mcp_result.content.is_object() || mcp_result.content.is_array() {
                ToolResult::success_with_data(content.clone(), mcp_result.content)
            } else {
                ToolResult::success(content)
            }
        } else {
            ToolResult::error(mcp_result.error.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }
}

#[async_trait]
impl AgentTool for McpToolBridge {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn definition(&self) -> ToolDefinition {
        self.to_tool_definition()
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse JSON arguments
        let args_value: serde_json::Value = serde_json::from_str(args).map_err(|e| {
            AetherError::InvalidConfig {
                message: format!("Invalid JSON arguments for tool '{}': {}", self.tool_def.name, e),
                suggestion: Some("Ensure arguments are valid JSON".to_string()),
            }
        })?;

        // Call through MCP client
        let mcp_result = self
            .client
            .call_tool(&self.tool_def.name, args_value)
            .await?;

        Ok(Self::convert_result(mcp_result))
    }

    fn requires_confirmation(&self) -> bool {
        self.tool_def.requires_confirmation
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }
}

/// Helper to create bridges from a client without consuming it
pub async fn create_bridges(client: &Arc<McpClient>) -> Vec<Arc<dyn AgentTool>> {
    McpToolBridge::from_client(Arc::clone(client))
        .await
        .into_iter()
        .map(|b| Arc::new(b) as Arc<dyn AgentTool>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::types::McpToolResult;
    use serde_json::json;

    fn create_test_tool() -> McpTool {
        McpTool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            requires_confirmation: false,
        }
    }

    #[test]
    fn test_bridge_to_definition() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "test-server".to_string());

        let def = bridge.definition();
        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(!def.requires_confirmation);
        // All MCP tools are categorized as Mcp
        assert_eq!(def.category, ToolCategory::Mcp);
    }

    #[test]
    fn test_bridge_category() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "some-server".to_string());

        // External MCP tools are categorized as Mcp
        assert_eq!(bridge.category(), ToolCategory::Mcp);
    }

    #[test]
    fn test_convert_result_success_string() {
        let mcp_result = McpToolResult::success(json!("Hello, world!"));
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(result.success);
        assert_eq!(result.content, "Hello, world!");
        assert!(result.data.is_none());
    }

    #[test]
    fn test_convert_result_success_object() {
        let mcp_result = McpToolResult::success(json!({"status": "ok", "count": 42}));
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(result.success);
        assert!(result.data.is_some());

        let data = result.data.unwrap();
        assert_eq!(data["status"], "ok");
        assert_eq!(data["count"], 42);
    }

    #[test]
    fn test_convert_result_error() {
        let mcp_result = McpToolResult::error("Something went wrong");
        let result = McpToolBridge::convert_result(mcp_result);

        assert!(!result.success);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_bridge_name() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "test-server".to_string());

        assert_eq!(bridge.name(), "test_tool");
    }

    #[test]
    fn test_bridge_server_name() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "my-server".to_string());

        assert_eq!(bridge.server_name(), "my-server");
    }

    #[test]
    fn test_bridge_requires_confirmation_false() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "test-server".to_string());

        assert!(!bridge.requires_confirmation());
    }

    #[test]
    fn test_bridge_requires_confirmation_true() {
        let mut tool = create_test_tool();
        tool.requires_confirmation = true;

        let client = Arc::new(McpClient::new());
        let bridge = McpToolBridge::new(tool, client, "shell-server".to_string());

        assert!(bridge.requires_confirmation());
    }

    #[tokio::test]
    async fn test_from_client_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = McpToolBridge::from_client(client).await;

        // Empty client has no tools
        assert!(bridges.is_empty());
    }

    #[tokio::test]
    async fn test_create_bridges_empty() {
        let client = Arc::new(McpClient::new());
        let bridges = create_bridges(&client).await;

        assert!(bridges.is_empty());
    }
}

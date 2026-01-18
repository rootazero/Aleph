//! MCP Tool Wrapper for rig-core
//!
//! Wraps external MCP server tools as rig-compatible tools for dynamic registration.
//!
//! This allows MCP tools to be added to the ToolServerHandle at runtime (hot-reload).

use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};

use crate::mcp::{McpClient, McpTool};

/// Wrapper for MCP tools that implements rig's ToolDyn trait
///
/// This enables MCP tools from external servers to be dynamically
/// registered with the rig-core ToolServer at runtime.
pub struct McpToolWrapper {
    /// Tool definition from MCP server
    tool_def: McpTool,
    /// MCP client for executing tool calls
    client: Arc<McpClient>,
    /// Server name for identification
    server_name: String,
}

impl McpToolWrapper {
    /// Create a new MCP tool wrapper
    ///
    /// # Arguments
    /// * `tool_def` - Tool definition from MCP server
    /// * `client` - Arc reference to MCP client
    /// * `server_name` - Name of the MCP server
    pub fn new(tool_def: McpTool, client: Arc<McpClient>, server_name: String) -> Self {
        Self {
            tool_def,
            client,
            server_name,
        }
    }

    /// Create wrappers for all tools from an MCP client
    ///
    /// This creates `McpToolWrapper` instances for all registered tools
    /// from an MCP client, ready for hot-reload registration.
    pub async fn from_client(client: Arc<McpClient>) -> Vec<Self> {
        let mut wrappers = Vec::new();

        let all_tools = client.list_tools().await;
        for tool in all_tools {
            // External tools have format "server_name:tool_name"
            let server_name = tool.name.split(':').next().unwrap_or("unknown").to_string();

            wrappers.push(Self::new(tool, Arc::clone(&client), server_name));
        }

        wrappers
    }

    /// Get the server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the tool name
    pub fn tool_name(&self) -> &str {
        &self.tool_def.name
    }
}

// Implement rig's ToolDyn trait for dynamic dispatch
impl ToolDyn for McpToolWrapper {
    fn name(&self) -> String {
        self.tool_def.name.clone()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> Pin<Box<dyn Future<Output = ToolDefinition> + Send + 'a>> {
        Box::pin(async move {
            ToolDefinition {
                name: self.tool_def.name.clone(),
                description: self.tool_def.description.clone(),
                parameters: self.tool_def.input_schema.clone(),
            }
        })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // Parse JSON arguments
            let args_value: serde_json::Value =
                serde_json::from_str(&args).map_err(ToolError::JsonError)?;

            // Call through MCP client
            let mcp_result = self
                .client
                .call_tool(&self.tool_def.name, args_value)
                .await
                .map_err(|e: crate::error::AetherError| {
                    ToolError::ToolCallError(Box::new(McpToolWrapperError(e.to_string())))
                })?;

            // Convert result to string
            if mcp_result.success {
                match &mcp_result.content {
                    serde_json::Value::String(s) => Ok(s.clone()),
                    serde_json::Value::Null => Ok(String::new()),
                    other => serde_json::to_string(other).map_err(ToolError::JsonError),
                }
            } else {
                Err(ToolError::ToolCallError(Box::new(McpToolWrapperError(
                    mcp_result
                        .error
                        .unwrap_or_else(|| "Unknown MCP tool error".to_string()),
                ))))
            }
        })
    }
}

/// Error type for MCP tool wrapper
#[derive(Debug)]
pub struct McpToolWrapperError(String);

impl std::fmt::Display for McpToolWrapperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP tool error: {}", self.0)
    }
}

impl std::error::Error for McpToolWrapperError {}

// Ensure Send + Sync for thread safety (required by ToolDyn)
unsafe impl Send for McpToolWrapper {}
unsafe impl Sync for McpToolWrapper {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpTool;
    use serde_json::json;

    fn create_test_tool() -> McpTool {
        McpTool {
            name: "test:read_file".to_string(),
            description: "Read a file from the filesystem".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to read" }
                },
                "required": ["path"]
            }),
            requires_confirmation: false,
        }
    }

    #[test]
    fn test_wrapper_name() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let wrapper = McpToolWrapper::new(tool, client, "test".to_string());

        assert_eq!(wrapper.name(), "test:read_file");
        assert_eq!(wrapper.tool_name(), "test:read_file");
        assert_eq!(wrapper.server_name(), "test");
    }

    #[tokio::test]
    async fn test_wrapper_definition() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let wrapper = McpToolWrapper::new(tool, client, "test".to_string());

        let def = wrapper.definition("".to_string()).await;
        assert_eq!(def.name, "test:read_file");
        assert_eq!(def.description, "Read a file from the filesystem");
        assert!(def.parameters.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_from_client_empty() {
        let client = Arc::new(McpClient::new());
        let wrappers = McpToolWrapper::from_client(client).await;

        // Empty client has no tools
        assert!(wrappers.is_empty());
    }
}

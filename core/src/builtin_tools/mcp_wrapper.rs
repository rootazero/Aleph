//! MCP Tool Wrapper
//!
//! Wraps external MCP server tools for dynamic registration.
//!
//! This allows MCP tools to be added to the AlephToolServerHandle at runtime (hot-reload).

use std::pin::Pin;
use crate::sync_primitives::Arc;

use futures::Future;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::mcp::{McpClient, McpTool};
use crate::tools::AlephToolDyn;

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

// Implement AlephToolDyn trait for dynamic dispatch
impl AlephToolDyn for McpToolWrapper {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            &self.tool_def.name,
            &self.tool_def.description,
            self.tool_def.input_schema.clone(),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            // Call through MCP client
            let mcp_result = self.client.call_tool(&self.tool_def.name, args).await?;

            // Convert result to Value
            if mcp_result.success {
                Ok(mcp_result.content)
            } else {
                Err(crate::error::AlephError::tool(
                    mcp_result
                        .error
                        .unwrap_or_else(|| "Unknown MCP tool error".to_string()),
                ))
            }
        })
    }
}

// SAFETY: McpToolWrapper is Send + Sync because:
// - tool_def (McpTool) contains only String and serde_json::Value (both Send + Sync)
// - client (Arc<McpClient>) is Send + Sync (Arc<T> is Send + Sync when T: Send + Sync,
//   and McpClient uses RwLock for interior mutability which is Send + Sync)
// - server_name (String) is Send + Sync
// This is required by AlephToolDyn trait which has Send + Sync bounds.
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

    #[test]
    fn test_wrapper_definition() {
        let tool = create_test_tool();
        let client = Arc::new(McpClient::new());
        let wrapper = McpToolWrapper::new(tool, client, "test".to_string());

        let def = wrapper.definition();
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

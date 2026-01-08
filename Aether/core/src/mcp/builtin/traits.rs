//! Builtin MCP Service Trait
//!
//! Defines the interface for builtin MCP services.

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::types::{McpResource, McpTool, McpToolResult};

/// Trait for builtin MCP services
///
/// All builtin services implement this trait to provide a consistent interface
/// for tool discovery, execution, and resource management.
#[async_trait]
pub trait BuiltinMcpService: Send + Sync {
    /// Get the service identifier (e.g., "builtin:fs", "builtin:git")
    fn name(&self) -> &str;

    /// Get human-readable description of the service
    fn description(&self) -> &str;

    /// List available resources provided by this service
    async fn list_resources(&self) -> Result<Vec<McpResource>>;

    /// Read a resource by URI
    async fn read_resource(&self, uri: &str) -> Result<String>;

    /// List available tools provided by this service
    fn list_tools(&self) -> Vec<McpTool>;

    /// Execute a tool with the given arguments
    ///
    /// # Arguments
    /// * `name` - Tool name (without service prefix)
    /// * `args` - JSON arguments for the tool
    ///
    /// # Returns
    /// Tool execution result
    async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<McpToolResult>;

    /// Check if a tool requires user confirmation before execution
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool to check
    ///
    /// # Returns
    /// true if the tool requires confirmation
    fn requires_confirmation(&self, tool_name: &str) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    /// Mock service for testing
    struct MockService;

    #[async_trait]
    impl BuiltinMcpService for MockService {
        fn name(&self) -> &str {
            "builtin:mock"
        }

        fn description(&self) -> &str {
            "Mock service for testing"
        }

        async fn list_resources(&self) -> Result<Vec<McpResource>> {
            Ok(vec![])
        }

        async fn read_resource(&self, _uri: &str) -> Result<String> {
            Ok("mock content".to_string())
        }

        fn list_tools(&self) -> Vec<McpTool> {
            vec![McpTool {
                name: "mock_tool".to_string(),
                description: "A mock tool".to_string(),
                input_schema: json!({"type": "object"}),
                requires_confirmation: false,
            }]
        }

        async fn call_tool(&self, name: &str, _args: serde_json::Value) -> Result<McpToolResult> {
            Ok(McpToolResult::success(json!({
                "tool": name,
                "result": "ok"
            })))
        }

        fn requires_confirmation(&self, _tool_name: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn test_mock_service() {
        let service: Arc<dyn BuiltinMcpService> = Arc::new(MockService);

        assert_eq!(service.name(), "builtin:mock");
        assert_eq!(service.list_tools().len(), 1);

        let result = service.call_tool("mock_tool", json!({})).await.unwrap();
        assert!(result.success);
    }
}

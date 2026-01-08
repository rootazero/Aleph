//! System Tool Trait
//!
//! Defines the interface for system tools (Tier 1 built-in tools).
//! These are native Rust implementations exposed via MCP-like JSON interface.

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::types::{McpResource, McpTool, McpToolResult};

/// Trait for system tools (Tier 1 built-in tools)
///
/// All system tools implement this trait to provide a consistent interface
/// for tool discovery, execution, and resource management.
///
/// System tools are:
/// - Native Rust code (not external processes)
/// - Always available (no installation required)
/// - Top-level commands (/fs, /git, /sys, /shell)
#[async_trait]
pub trait SystemTool: Send + Sync {
    /// Get the tool identifier (e.g., "fs", "git", "sys", "shell")
    fn name(&self) -> &str;

    /// Get human-readable description of the tool
    fn description(&self) -> &str;

    /// List available resources provided by this tool
    async fn list_resources(&self) -> Result<Vec<McpResource>>;

    /// Read a resource by URI
    async fn read_resource(&self, uri: &str) -> Result<String>;

    /// List available sub-tools provided by this tool
    fn list_tools(&self) -> Vec<McpTool>;

    /// Execute a sub-tool with the given arguments
    ///
    /// # Arguments
    /// * `name` - Sub-tool name (e.g., "read", "status")
    /// * `args` - JSON arguments for the tool
    ///
    /// # Returns
    /// Tool execution result
    async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<McpToolResult>;

    /// Check if a sub-tool requires user confirmation before execution
    ///
    /// # Arguments
    /// * `tool_name` - Name of the sub-tool to check
    ///
    /// # Returns
    /// true if the tool requires confirmation
    fn requires_confirmation(&self, tool_name: &str) -> bool;
}

// Type alias for backward compatibility
pub type BuiltinMcpService = dyn SystemTool;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    /// Mock tool for testing
    struct MockTool;

    #[async_trait]
    impl SystemTool for MockTool {
        fn name(&self) -> &str {
            "mock"
        }

        fn description(&self) -> &str {
            "Mock tool for testing"
        }

        async fn list_resources(&self) -> Result<Vec<McpResource>> {
            Ok(vec![])
        }

        async fn read_resource(&self, _uri: &str) -> Result<String> {
            Ok("mock content".to_string())
        }

        fn list_tools(&self) -> Vec<McpTool> {
            vec![McpTool {
                name: "mock_action".to_string(),
                description: "A mock action".to_string(),
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
    async fn test_mock_tool() {
        let tool: Arc<dyn SystemTool> = Arc::new(MockTool);

        assert_eq!(tool.name(), "mock");
        assert_eq!(tool.list_tools().len(), 1);

        let result = tool.call_tool("mock_action", json!({})).await.unwrap();
        assert!(result.success);
    }
}

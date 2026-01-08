//! MCP Type Definitions
//!
//! Common types used across MCP services.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Unique tool name (e.g., "file_read", "git_status")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: Value,
    /// Whether this tool requires user confirmation before execution
    pub requires_confirmation: bool,
}

/// MCP Tool Call request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCall {
    /// Tool name to invoke
    pub name: String,
    /// Tool arguments as JSON
    pub arguments: Value,
}

/// MCP Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// Whether the tool executed successfully
    pub success: bool,
    /// Result content (JSON)
    pub content: Value,
    /// Error message if failed
    pub error: Option<String>,
}

impl McpToolResult {
    /// Create a successful result
    pub fn success(content: Value) -> Self {
        Self {
            success: true,
            content,
            error: None,
        }
    }

    /// Create a failed result
    pub fn error<S: Into<String>>(message: S) -> Self {
        Self {
            success: false,
            content: Value::Null,
            error: Some(message.into()),
        }
    }
}

/// MCP Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Resource URI (e.g., "file:///path/to/file")
    pub uri: String,
    /// Human-readable name
    pub name: String,
    /// Resource description
    pub description: Option<String>,
    /// MIME type if known
    pub mime_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_result_success() {
        let result = McpToolResult::success(json!({"data": "test"}));
        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.content, json!({"data": "test"}));
    }

    #[test]
    fn test_tool_result_error() {
        let result = McpToolResult::error("Something went wrong");
        assert!(!result.success);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }
}

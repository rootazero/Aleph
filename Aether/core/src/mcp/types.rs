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

// ===== UniFFI Types for Swift UI =====

/// MCP service information for UI display
#[derive(Debug, Clone)]
pub struct McpServiceInfo {
    /// Service name (e.g., "fs", "git", "shell")
    pub name: String,
    /// Service description
    pub description: String,
    /// Whether this is a builtin service
    pub is_builtin: bool,
    /// Whether the service is currently running
    pub is_running: bool,
    /// Number of tools provided by this service
    pub tool_count: u32,
}

/// MCP tool information for UI display
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Tool name (e.g., "fs:read_file", "git:status")
    pub name: String,
    /// Tool description
    pub description: String,
    /// Whether this tool requires user confirmation
    pub requires_confirmation: bool,
    /// Parent service name
    pub service_name: String,
}

/// MCP configuration for Settings UI
#[derive(Debug, Clone)]
pub struct McpSettingsConfig {
    /// MCP capability enabled
    pub enabled: bool,
    /// Filesystem service enabled
    pub fs_enabled: bool,
    /// Git service enabled
    pub git_enabled: bool,
    /// Shell service enabled
    pub shell_enabled: bool,
    /// System info service enabled
    pub system_info_enabled: bool,
    /// Allowed filesystem roots
    pub allowed_roots: Vec<String>,
    /// Allowed git repositories
    pub allowed_repos: Vec<String>,
    /// Allowed shell commands
    pub allowed_commands: Vec<String>,
    /// Shell command timeout
    pub shell_timeout_seconds: u64,
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

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

// ===== Remote Server Configuration =====

/// Transport preference for remote MCP servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportPreference {
    /// Automatically select transport (HTTP for simple servers, SSE if notifications needed)
    #[default]
    Auto,
    /// Force HTTP transport (no server-initiated notifications)
    Http,
    /// Force SSE transport (supports server-initiated notifications)
    Sse,
}

/// Remote MCP server configuration
///
/// Used to configure connections to remote MCP servers accessible over HTTP/HTTPS.
/// Unlike local servers (which use stdio), remote servers communicate via HTTP POST
/// requests and optionally SSE for server-initiated notifications.
#[derive(Debug, Clone)]
pub struct McpRemoteServerConfig {
    /// Server name (used for identification and logging)
    pub name: String,
    /// Server URL (e.g., "https://api.example.com/mcp")
    pub url: String,
    /// Custom HTTP headers (for authorization tokens, API keys, etc.)
    pub headers: std::collections::HashMap<String, String>,
    /// Transport preference (Auto, Http, or Sse)
    pub transport: TransportPreference,
    /// Request timeout in seconds (default: 30)
    pub timeout_seconds: Option<u64>,
}

impl McpRemoteServerConfig {
    /// Create a new remote server configuration
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            headers: std::collections::HashMap::new(),
            transport: TransportPreference::Auto,
            timeout_seconds: None,
        }
    }

    /// Add an authorization header
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.headers
            .insert("Authorization".to_string(), format!("Bearer {}", token.into()));
        self
    }

    /// Add a custom header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set transport preference
    pub fn with_transport(mut self, transport: TransportPreference) -> Self {
        self.transport = transport;
        self
    }

    /// Set request timeout
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }
}

// ===== UniFFI Types for Swift UI =====

/// MCP server type (builtin or external)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerType {
    /// Builtin service (Rust native implementation)
    Builtin,
    /// External server (user installed)
    External,
}

/// MCP server status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    /// Server is stopped
    Stopped,
    /// Server is starting
    Starting,
    /// Server is running
    Running,
    /// Server has an error
    Error,
}

/// MCP server permissions configuration
#[derive(Debug, Clone, Default)]
pub struct McpServerPermissions {
    /// Whether this server requires user confirmation before tool execution
    pub requires_confirmation: bool,
    /// Allowed file paths (for file operations)
    pub allowed_paths: Vec<String>,
    /// Allowed commands (for shell operations)
    pub allowed_commands: Vec<String>,
}

/// MCP server configuration (for both builtin and external servers)
/// This is the UI-facing configuration structure
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Server unique identifier
    pub id: String,
    /// Server display name
    pub name: String,
    /// Server type (builtin or external)
    pub server_type: McpServerType,
    /// Whether the server is enabled
    pub enabled: bool,
    /// Command to execute (external servers only)
    pub command: Option<String>,
    /// Command arguments (external servers only)
    pub args: Vec<String>,
    /// Environment variables (key-value pairs)
    pub env: Vec<McpEnvVar>,
    /// Working directory (external servers only)
    pub working_directory: Option<String>,
    /// Trigger command in Halo (e.g., /git for System Tools, /mcp/server for External)
    pub trigger_command: Option<String>,
    /// Server permissions
    pub permissions: McpServerPermissions,
    /// SF Symbol icon name
    pub icon: String,
    /// Theme color (hex)
    pub color: String,
}

/// Environment variable key-value pair (for UniFFI)
#[derive(Debug, Clone)]
pub struct McpEnvVar {
    pub key: String,
    pub value: String,
}

/// MCP server status info (for UI display)
#[derive(Debug, Clone)]
pub struct McpServerStatusInfo {
    /// Current status
    pub status: McpServerStatus,
    /// Status message
    pub message: Option<String>,
    /// Last error message (if any)
    pub last_error: Option<String>,
}

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

    #[test]
    fn test_remote_server_config() {
        let config = McpRemoteServerConfig {
            name: "remote-test".to_string(),
            url: "https://example.com/mcp".to_string(),
            headers: std::collections::HashMap::new(),
            transport: TransportPreference::Auto,
            timeout_seconds: Some(300),
        };

        assert_eq!(config.name, "remote-test");
        assert_eq!(config.url, "https://example.com/mcp");
        assert!(matches!(config.transport, TransportPreference::Auto));
    }

    #[test]
    fn test_remote_server_config_builder() {
        let config = McpRemoteServerConfig::new("my-server", "https://api.example.com/mcp")
            .with_bearer_token("secret-token")
            .with_header("X-Custom", "value")
            .with_transport(TransportPreference::Sse)
            .with_timeout(60);

        assert_eq!(config.name, "my-server");
        assert_eq!(config.url, "https://api.example.com/mcp");
        assert_eq!(
            config.headers.get("Authorization"),
            Some(&"Bearer secret-token".to_string())
        );
        assert_eq!(
            config.headers.get("X-Custom"),
            Some(&"value".to_string())
        );
        assert!(matches!(config.transport, TransportPreference::Sse));
        assert_eq!(config.timeout_seconds, Some(60));
    }

    #[test]
    fn test_transport_preference_default() {
        let pref = TransportPreference::default();
        assert!(matches!(pref, TransportPreference::Auto));
    }
}

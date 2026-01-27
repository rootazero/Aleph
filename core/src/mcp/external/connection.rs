//! MCP Server Connection
//!
//! Manages the lifecycle and communication with an external MCP server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{AetherError, Result};
use crate::mcp::jsonrpc::{mcp as mcp_types, IdGenerator, JsonRpcNotification, JsonRpcRequest};
use crate::mcp::transport::{McpTransport, StdioTransport};
use crate::mcp::types::McpTool;

/// Default timeout for the entire MCP server connection process
/// This includes: process spawn + initialize handshake + tools/list
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected
    Disconnected,
    /// Connection in progress
    Connecting,
    /// Connected and ready
    Ready,
    /// Connection failed
    Failed,
}

/// External MCP server connection
///
/// This struct manages the lifecycle and communication with an MCP server.
/// It uses a trait object (`Box<dyn McpTransport>`) to support different
/// transport implementations (stdio, HTTP, SSE).
pub struct McpServerConnection {
    /// Server name
    name: String,
    /// Transport layer (trait object for flexibility)
    transport: Box<dyn McpTransport>,
    /// Request ID generator
    id_gen: IdGenerator,
    /// Server capabilities (after initialize)
    capabilities: RwLock<Option<mcp_types::ServerCapabilities>>,
    /// Cached tools list
    cached_tools: RwLock<Vec<McpTool>>,
    /// Connection state
    state: RwLock<ConnectionState>,
}

impl McpServerConnection {
    /// Connect to an external MCP server with timeout protection
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `command` - Command to execute
    /// * `args` - Command arguments
    /// * `env` - Environment variables
    /// * `cwd` - Working directory
    /// * `timeout` - Connection timeout (defaults to 30s). This covers the entire
    ///   connection process: process spawn + initialize handshake + tools/list
    pub async fn connect(
        name: impl Into<String>,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        let name = name.into();
        let connect_timeout = timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT);

        // Wrap entire connection process with timeout
        tokio::time::timeout(
            connect_timeout,
            Self::connect_internal(&name, command, args, env, cwd, timeout),
        )
        .await
        .map_err(|_| {
            AetherError::Timeout {
                suggestion: Some(format!(
                    "MCP server '{}' connection timed out after {}s. Check if the server is installed and responding.",
                    name,
                    connect_timeout.as_secs()
                )),
            }
        })?
    }

    /// Internal connection logic (without timeout wrapper)
    async fn connect_internal(
        name: &str,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        // Spawn the server process
        let mut transport = StdioTransport::spawn(name, command, args, env, cwd).await?;

        // Set per-request timeout if provided
        if let Some(t) = timeout {
            transport = transport.with_timeout(t);
        }

        Self::with_transport(name, Box::new(transport)).await
    }

    /// Create a connection with a custom transport
    ///
    /// This constructor allows creating connections with any transport implementation,
    /// enabling support for HTTP, SSE, or mock transports for testing.
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `transport` - A boxed transport implementing `McpTransport`
    ///
    /// # Example
    /// ```ignore
    /// let http_transport = HttpTransport::new("https://mcp.example.com").await?;
    /// let conn = McpServerConnection::with_transport("remote-server", Box::new(http_transport)).await?;
    /// ```
    pub async fn with_transport(
        name: impl Into<String>,
        transport: Box<dyn McpTransport>,
    ) -> Result<Self> {
        let name = name.into();
        let conn = Self {
            name: name.clone(),
            transport,
            id_gen: IdGenerator::new(),
            capabilities: RwLock::new(None),
            cached_tools: RwLock::new(Vec::new()),
            state: RwLock::new(ConnectionState::Connecting),
        };

        // Perform MCP initialize handshake
        conn.initialize().await?;

        Ok(conn)
    }

    /// Perform MCP initialize handshake
    async fn initialize(&self) -> Result<()> {
        let params = mcp_types::InitializeParams::aether_default();
        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "initialize",
            serde_json::to_value(&params).map_err(|e| {
                AetherError::IoError(format!("Failed to serialize initialize params: {}", e))
            })?,
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "MCP server '{}' initialize failed: {}",
                self.name, e
            ))
        })?;

        // Parse initialize result
        let init_result: mcp_types::InitializeResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
                    "Failed to parse initialize result from '{}': {}",
                    self.name, e
                ))
            })?;

        tracing::info!(
            server = %self.name,
            protocol = %init_result.protocol_version,
            server_name = ?init_result.server_info.as_ref().map(|i| &i.name),
            "MCP server initialized"
        );

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            *caps = Some(init_result.capabilities);
        }

        // Send initialized notification (per JSON-RPC spec, notifications have no id)
        let notification = JsonRpcNotification::new("notifications/initialized");
        if let Err(e) = self.transport.send_notification(&notification).await {
            tracing::warn!(
                server = %self.name,
                error = %e,
                "Failed to send initialized notification (non-fatal)"
            );
        }

        // Update state
        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Ready;
        }

        // Pre-fetch tools list
        self.refresh_tools().await?;

        Ok(())
    }

    /// Refresh the cached tools list
    pub async fn refresh_tools(&self) -> Result<()> {
        let request = JsonRpcRequest::new(self.id_gen.next(), "tools/list");
        let response = self.transport.send_request(&request).await?;

        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "MCP server '{}' tools/list failed: {}",
                self.name, e
            ))
        })?;

        let tools_result: mcp_types::ToolsListResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
                    "Failed to parse tools list from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert to our McpTool format
        let tools: Vec<McpTool> = tools_result
            .tools
            .into_iter()
            .map(|t| McpTool {
                name: format!("{}:{}", self.name, t.name), // Namespace with server name
                description: t.description.unwrap_or_default(),
                input_schema: t.input_schema.unwrap_or(json!({"type": "object"})),
                requires_confirmation: false, // External tools default to no confirmation
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            tool_count = tools.len(),
            "Cached tools list"
        );

        // Update cache
        {
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }

        Ok(())
    }

    /// Get cached tools list
    pub async fn list_tools(&self) -> Vec<McpTool> {
        self.cached_tools.read().await.clone()
    }

    /// Check if this connection provides a specific tool
    pub async fn has_tool(&self, name: &str) -> bool {
        // Check with and without namespace prefix
        let full_name = if name.starts_with(&format!("{}:", self.name)) {
            name.to_string()
        } else {
            format!("{}:{}", self.name, name)
        };

        self.cached_tools
            .read()
            .await
            .iter()
            .any(|t| t.name == full_name || t.name == name)
    }

    /// Call a tool on this server
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        // Strip server namespace prefix if present
        let tool_name = name
            .strip_prefix(&format!("{}:", self.name))
            .unwrap_or(name);

        let params = mcp_types::ToolCallParams {
            name: tool_name.to_string(),
            arguments: Some(arguments),
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "tools/call",
            serde_json::to_value(&params).map_err(|e| {
                AetherError::IoError(format!("Failed to serialize tool call params: {}", e))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            tool = %tool_name,
            "Calling tool"
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "Tool call '{}' on '{}' failed: {}",
                tool_name, self.name, e
            ))
        })?;

        // Parse tool call result
        let call_result: mcp_types::ToolCallResult = serde_json::from_value(result.clone())
            .unwrap_or(mcp_types::ToolCallResult {
                content: vec![mcp_types::ToolResultContent::Text {
                    text: result.to_string(),
                }],
                is_error: None,
            });

        // Convert result to Value
        if call_result.is_error == Some(true) {
            let error_text = call_result
                .content
                .into_iter()
                .filter_map(|c| match c {
                    mcp_types::ToolResultContent::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            return Err(AetherError::IoError(format!(
                "Tool '{}' returned error: {}",
                tool_name, error_text
            )));
        }

        // Extract text content from result
        let content: Vec<Value> = call_result
            .content
            .into_iter()
            .map(|c| match c {
                mcp_types::ToolResultContent::Text { text } => {
                    json!({"type": "text", "text": text})
                }
                mcp_types::ToolResultContent::Image { data, mime_type } => {
                    json!({"type": "image", "data": data, "mimeType": mime_type})
                }
                mcp_types::ToolResultContent::Resource { uri, text } => {
                    json!({"type": "resource", "uri": uri, "text": text})
                }
            })
            .collect();

        Ok(json!({
            "content": content,
        }))
    }

    /// Get current connection state
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Get server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if server is running
    pub async fn is_running(&self) -> bool {
        self.transport.is_alive().await
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        tracing::info!(server = %self.name, "Closing MCP connection");

        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Disconnected;
        }

        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most tests require an actual MCP server to be available
    // These are basic structure tests

    #[test]
    fn test_connection_state() {
        assert_eq!(ConnectionState::Disconnected, ConnectionState::Disconnected);
        assert_ne!(ConnectionState::Ready, ConnectionState::Failed);
    }

    #[tokio::test]
    async fn test_connect_nonexistent() {
        let result = McpServerConnection::connect(
            "test-fail",
            "/nonexistent/mcp/server",
            &[],
            &HashMap::new(),
            None,
            None,
        )
        .await;

        assert!(result.is_err());
    }
}

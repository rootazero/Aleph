//! MCP Client - External Server Registry
//!
//! Manages external MCP server connections only.
//! Native tools (fs, git, shell, etc.) are now handled via AgentTool infrastructure.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::error::{AetherError, Result};
use crate::mcp::external::{check_runtime, McpServerConnection, RuntimeKind};
use crate::mcp::types::{McpTool, McpToolResult};

/// External server configuration
#[derive(Debug, Clone)]
pub struct ExternalServerConfig {
    /// Server name
    pub name: String,
    /// Command to execute
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Required runtime (node, python, bun, etc.)
    pub requires_runtime: Option<String>,
    /// Request timeout in seconds
    pub timeout_seconds: Option<u64>,
}

/// Tool location - where a tool comes from
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum ToolLocation {
    /// External server (name)
    External(String),
}

/// MCP Client - registry for external MCP server connections
///
/// Note: Native tools (fs, git, shell, etc.) are now handled via
/// the AgentTool infrastructure in the `tools` module. This client
/// only manages external MCP server connections.
pub struct McpClient {
    /// Tool name to location mapping (RwLock for thread-safe updates)
    tool_location_map: RwLock<HashMap<String, ToolLocation>>,
    /// External server connections
    external_servers: RwLock<HashMap<String, Arc<McpServerConnection>>>,
}

impl McpClient {
    /// Create a new empty MCP client
    pub fn new() -> Self {
        Self {
            tool_location_map: RwLock::new(HashMap::new()),
            external_servers: RwLock::new(HashMap::new()),
        }
    }

    /// Start external MCP servers
    ///
    /// Checks runtime availability before starting each server.
    /// Servers with missing runtimes are skipped with a warning.
    pub async fn start_external_servers(&self, configs: Vec<ExternalServerConfig>) -> Result<()> {
        for config in configs {
            // Check runtime if required
            if let Some(ref runtime_str) = config.requires_runtime {
                let runtime = RuntimeKind::from_str(runtime_str);
                if runtime != RuntimeKind::None {
                    let check = check_runtime(runtime);
                    if !check.available {
                        tracing::warn!(
                            server = %config.name,
                            runtime = %runtime,
                            "Skipping MCP server: {} not found",
                            runtime.display_name()
                        );
                        continue;
                    }
                    tracing::debug!(
                        server = %config.name,
                        runtime = %runtime,
                        version = ?check.version,
                        "Runtime check passed"
                    );
                }
            }

            // Start the server
            match self.start_external_server(config.clone()).await {
                Ok(()) => {
                    tracing::info!(
                        server = %config.name,
                        "External MCP server started"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        server = %config.name,
                        error = %e,
                        "Failed to start external MCP server"
                    );
                }
            }
        }

        Ok(())
    }

    /// Start a single external server
    async fn start_external_server(&self, config: ExternalServerConfig) -> Result<()> {
        let timeout = config.timeout_seconds.map(Duration::from_secs);

        let connection = McpServerConnection::connect(
            &config.name,
            &config.command,
            &config.args,
            &config.env,
            config.cwd.as_ref(),
            timeout,
        )
        .await?;

        let connection = Arc::new(connection);

        // Register tools from this server (thread-safe via RwLock)
        let tools = connection.list_tools().await;
        {
            let mut map = self.tool_location_map.write().await;
            for tool in &tools {
                map.insert(tool.name.clone(), ToolLocation::External(config.name.clone()));
            }
        }

        // Store connection
        {
            let mut servers = self.external_servers.write().await;
            servers.insert(config.name, connection);
        }

        Ok(())
    }

    /// List all available tools from external servers
    pub async fn list_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();

        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            tools.extend(connection.list_tools().await);
        }

        tools
    }

    /// Get tools as a formatted list for context injection
    pub async fn get_tools_for_context(&self) -> Vec<(String, String, serde_json::Value)> {
        self.list_tools()
            .await
            .into_iter()
            .map(|t| (t.name, t.description, t.input_schema))
            .collect()
    }

    /// Check if a tool requires confirmation
    pub async fn requires_confirmation(&self, tool_name: &str) -> bool {
        let map = self.tool_location_map.read().await;
        if map.get(tool_name).is_some() {
            // External tools don't require confirmation by default
            // Could be configurable in the future
            return false;
        }
        // Default to requiring confirmation for unknown tools
        true
    }

    /// Call a tool by name
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpToolResult> {
        let servers = self.external_servers.read().await;

        // Check if tool name has server prefix (e.g., "server_name:tool_name")
        if let Some((server_name, _tool_name)) = name.split_once(':') {
            if let Some(connection) = servers.get(server_name) {
                let result = connection.call_tool(name, args).await?;
                return Ok(McpToolResult::success(result));
            }
        }

        // Try all servers
        for connection in servers.values() {
            if connection.has_tool(name).await {
                let result = connection.call_tool(name, args).await?;
                return Ok(McpToolResult::success(result));
            }
        }

        // Tool not found
        Err(AetherError::McpToolNotFound(name.to_string()))
    }

    /// Get list of registered external server names
    pub async fn service_names(&self) -> Vec<String> {
        let servers = self.external_servers.read().await;
        servers.keys().cloned().collect()
    }

    /// Check if any external servers are connected
    pub async fn has_services(&self) -> bool {
        let servers = self.external_servers.read().await;
        !servers.is_empty()
    }

    /// Get total number of available tools from external servers
    pub async fn tool_count(&self) -> usize {
        self.list_tools().await.len()
    }

    /// Get number of external servers
    pub async fn external_server_count(&self) -> usize {
        self.external_servers.read().await.len()
    }

    /// Stop all external servers
    pub async fn stop_all(&self) -> Result<()> {
        let mut servers = self.external_servers.write().await;

        for (name, connection) in servers.drain() {
            tracing::info!(server = %name, "Stopping external MCP server");
            if let Err(e) = connection.close().await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Error stopping MCP server"
                );
            }
        }

        Ok(())
    }

    /// Check health of all external servers
    pub async fn check_server_health(&self) -> HashMap<String, bool> {
        let servers = self.external_servers.read().await;
        let mut health = HashMap::new();

        for (name, connection) in servers.iter() {
            health.insert(name.clone(), connection.is_running().await);
        }

        health
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating McpClient with configuration
pub struct McpClientBuilder {
    client: McpClient,
    external_configs: Vec<ExternalServerConfig>,
}

impl McpClientBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            client: McpClient::new(),
            external_configs: Vec::new(),
        }
    }

    /// Add an external server configuration
    pub fn with_external(mut self, config: ExternalServerConfig) -> Self {
        self.external_configs.push(config);
        self
    }

    /// Build the client (without starting external servers)
    pub fn build(self) -> McpClient {
        self.client
    }

    /// Build the client and start external servers
    pub async fn build_and_start(self) -> Result<McpClient> {
        let client = self.client;
        client.start_external_servers(self.external_configs).await?;
        Ok(client)
    }
}

impl Default for McpClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_client() {
        let client = McpClient::new();
        assert_eq!(client.tool_count().await, 0);
        assert!(!client.has_services().await);
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let client = McpClient::new();

        let result = client.call_tool("unknown_tool", serde_json::json!({})).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            AetherError::McpToolNotFound(name) => {
                assert_eq!(name, "unknown_tool");
            }
            _ => panic!("Expected McpToolNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_builder() {
        let client = McpClientBuilder::new().build();
        assert_eq!(client.external_server_count().await, 0);
    }

    #[tokio::test]
    async fn test_external_server_count() {
        let client = McpClient::new();
        assert_eq!(client.external_server_count().await, 0);
    }

    #[tokio::test]
    async fn test_stop_all_empty() {
        let client = McpClient::new();
        // Should not error when no servers to stop
        client.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_server_health_empty() {
        let client = McpClient::new();
        let health = client.check_server_health().await;
        assert!(health.is_empty());
    }

    #[tokio::test]
    async fn test_requires_confirmation_unknown() {
        let client = McpClient::new();
        // Unknown tools should require confirmation by default
        assert!(client.requires_confirmation("unknown").await);
    }
}

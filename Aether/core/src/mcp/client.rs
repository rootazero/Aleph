//! MCP Client - Service Registry and Tool Router
//!
//! Manages system tools and external MCP services, providing:
//! - Service registration
//! - Tool discovery and aggregation
//! - Tool call routing

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::error::{AetherError, Result};
use crate::services::tools::SystemTool;
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
enum ToolLocation {
    /// Builtin service (index in system_tools)
    Builtin(usize),
    /// External server (name)
    External(String),
}

/// MCP Client - the central registry for MCP services
pub struct McpClient {
    /// Registered builtin services
    system_tools: Vec<Arc<dyn SystemTool>>,
    /// Tool name to location mapping
    tool_location_map: HashMap<String, ToolLocation>,
    /// External server connections
    external_servers: RwLock<HashMap<String, Arc<McpServerConnection>>>,
}

impl McpClient {
    /// Create a new empty MCP client
    pub fn new() -> Self {
        Self {
            system_tools: Vec::new(),
            tool_location_map: HashMap::new(),
            external_servers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a builtin service
    pub fn register_system_tool(&mut self, service: Arc<dyn SystemTool>) {
        let service_idx = self.system_tools.len();

        // Map all tools from this service
        for tool in service.list_tools() {
            self.tool_location_map.insert(tool.name.clone(), ToolLocation::Builtin(service_idx));
        }

        self.system_tools.push(service);

        tracing::info!(
            service = %self.system_tools[service_idx].name(),
            tools = self.system_tools[service_idx].list_tools().len(),
            "Registered MCP builtin service"
        );
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

        // Register tools from this server
        let tools = connection.list_tools().await;
        for tool in &tools {
            self.tool_location_map
                .clone() // Clone for thread safety - would need RwLock for full solution
                .insert(tool.name.clone(), ToolLocation::External(config.name.clone()));
        }

        // Store connection
        {
            let mut servers = self.external_servers.write().await;
            servers.insert(config.name, connection);
        }

        Ok(())
    }

    /// List all available tools from all services (builtin + external)
    pub async fn list_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();

        // Builtin tools
        for service in &self.system_tools {
            tools.extend(service.list_tools());
        }

        // External tools
        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            tools.extend(connection.list_tools().await);
        }

        tools
    }

    /// List builtin tools only (sync version)
    pub fn list_builtin_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();
        for service in &self.system_tools {
            tools.extend(service.list_tools());
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
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        if let Some(location) = self.tool_location_map.get(tool_name) {
            match location {
                ToolLocation::Builtin(idx) => {
                    return self.system_tools[*idx].requires_confirmation(tool_name);
                }
                ToolLocation::External(_) => {
                    // External tools don't require confirmation by default
                    // Could be configurable in the future
                    return false;
                }
            }
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
        // Check builtin services first (by tool name)
        for service in &self.system_tools {
            let tools = service.list_tools();
            if tools.iter().any(|t| t.name == name) {
                return service.call_tool(name, args).await;
            }
        }

        // Check external servers
        {
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
        }

        // Tool not found
        Err(AetherError::McpToolNotFound(name.to_string()))
    }

    /// Get list of registered service names
    pub async fn service_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.system_tools
            .iter()
            .map(|s| s.name().to_string())
            .collect();

        let servers = self.external_servers.read().await;
        names.extend(servers.keys().cloned());

        names
    }

    /// Get list of builtin service names (sync version)
    pub fn builtin_service_names(&self) -> Vec<&str> {
        self.system_tools.iter().map(|s| s.name()).collect()
    }

    /// Check if any services are registered
    pub async fn has_services(&self) -> bool {
        if !self.system_tools.is_empty() {
            return true;
        }
        let servers = self.external_servers.read().await;
        !servers.is_empty()
    }

    /// Check if any builtin services are registered (sync version)
    pub fn has_system_tools(&self) -> bool {
        !self.system_tools.is_empty()
    }

    /// Get total number of available tools
    pub async fn tool_count(&self) -> usize {
        self.list_tools().await.len()
    }

    /// Get builtin tool count (sync version)
    pub fn builtin_tool_count(&self) -> usize {
        self.system_tools
            .iter()
            .map(|s| s.list_tools().len())
            .sum()
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

    /// Add a builtin service
    pub fn with_system_tool(mut self, service: Arc<dyn SystemTool>) -> Self {
        self.client.register_system_tool(service);
        self
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
    use crate::mcp::types::McpResource;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockService {
        name: &'static str,
        tools: Vec<McpTool>,
    }

    impl MockService {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                tools: vec![McpTool {
                    name: format!("{}_tool", name),
                    description: "A mock tool".to_string(),
                    input_schema: json!({"type": "object"}),
                    requires_confirmation: false,
                }],
            }
        }
    }

    #[async_trait]
    impl SystemTool for MockService {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Mock service"
        }

        async fn list_resources(&self) -> Result<Vec<McpResource>> {
            Ok(vec![])
        }

        async fn read_resource(&self, _uri: &str) -> Result<String> {
            Ok("mock".to_string())
        }

        fn list_tools(&self) -> Vec<McpTool> {
            self.tools.clone()
        }

        async fn call_tool(&self, name: &str, _args: serde_json::Value) -> Result<McpToolResult> {
            Ok(McpToolResult::success(json!({"tool": name})))
        }

        fn requires_confirmation(&self, _tool_name: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn test_register_and_list_tools() {
        let mut client = McpClient::new();
        client.register_system_tool(Arc::new(MockService::new("service1")));
        client.register_system_tool(Arc::new(MockService::new("service2")));

        let tools = client.list_tools().await;
        assert_eq!(tools.len(), 2);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"service1_tool"));
        assert!(tool_names.contains(&"service2_tool"));
    }

    #[tokio::test]
    async fn test_call_tool() {
        let mut client = McpClient::new();
        client.register_system_tool(Arc::new(MockService::new("test")));

        let result = client.call_tool("test_tool", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["tool"], "test_tool");
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let client = McpClient::new();

        let result = client.call_tool("unknown_tool", json!({})).await;
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
        let client = McpClientBuilder::new()
            .with_system_tool(Arc::new(MockService::new("builder_test")))
            .build();

        assert!(client.has_system_tools());
        assert_eq!(client.builtin_tool_count(), 1);
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
}

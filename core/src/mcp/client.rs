//! MCP Client - External Server Registry
//!
//! Manages external MCP server connections only.
//! Native tools (fs, git, shell, etc.) are now handled via AgentTool infrastructure.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::external::{check_runtime, McpServerConnection, RuntimeKind};
use crate::mcp::sampling::SamplingHandler;
use crate::mcp::transport::{HttpTransport, HttpTransportConfig, McpTransport, SseTransport, SseTransportConfig};
use crate::mcp::types::{McpRemoteServerConfig, McpTool, McpToolResult, TransportPreference};

/// MCP server startup report
///
/// Contains information about which servers started successfully
/// and which ones failed (with error messages).
#[derive(Debug, Clone, Default)]
pub struct McpStartupReport {
    /// Names of servers that started successfully
    pub succeeded: Vec<String>,
    /// Failed servers: (server_name, error_message)
    pub failed: Vec<(String, String)>,
}

impl McpStartupReport {
    /// Check if all servers started successfully
    pub fn all_succeeded(&self) -> bool {
        self.failed.is_empty()
    }

    /// Get total number of servers attempted
    pub fn total(&self) -> usize {
        self.succeeded.len() + self.failed.len()
    }
}

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
    /// Handler for sampling requests from servers
    sampling_handler: Arc<SamplingHandler>,
}

impl McpClient {
    /// Create a new empty MCP client
    pub fn new() -> Self {
        Self {
            tool_location_map: RwLock::new(HashMap::new()),
            external_servers: RwLock::new(HashMap::new()),
            sampling_handler: Arc::new(SamplingHandler::new()),
        }
    }

    /// Get the sampling handler
    pub fn sampling_handler(&self) -> &Arc<SamplingHandler> {
        &self.sampling_handler
    }

    /// Set callback for sampling requests
    pub async fn set_sampling_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(crate::mcp::jsonrpc::mcp::SamplingRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<crate::mcp::jsonrpc::mcp::SamplingResponse>> + Send + 'static,
    {
        self.sampling_handler.set_callback(callback).await;
    }

    /// Start external MCP servers concurrently
    ///
    /// Checks runtime availability before starting each server.
    /// Servers with missing runtimes are skipped with a warning.
    ///
    /// Returns a startup report with success/failure information for each server.
    /// This allows callers to handle partial failures appropriately.
    pub async fn start_external_servers(
        &self,
        configs: Vec<ExternalServerConfig>,
    ) -> McpStartupReport {
        // Pre-filter configs based on runtime availability (sync operation)
        let valid_configs: Vec<_> = configs
            .into_iter()
            .filter(|config| {
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
                            return false;
                        }
                        tracing::debug!(
                            server = %config.name,
                            runtime = %runtime,
                            version = ?check.version,
                            "Runtime check passed"
                        );
                    }
                }
                true
            })
            .collect();

        // Start all servers concurrently
        let futures: Vec<_> = valid_configs
            .into_iter()
            .map(|config| {
                let name = config.name.clone();
                async move {
                    let result = self.start_external_server(config).await;
                    (name, result)
                }
            })
            .collect();

        let results = join_all(futures).await;

        // Collect results into report
        let mut report = McpStartupReport::default();
        for (name, result) in results {
            match result {
                Ok(()) => {
                    tracing::info!(server = %name, "External MCP server started");
                    report.succeeded.push(name);
                }
                Err(e) => {
                    tracing::error!(server = %name, error = %e, "Failed to start external MCP server");
                    report.failed.push((name, e.to_string()));
                }
            }
        }

        report
    }

    /// Start a single external server
    ///
    /// This method is public to support incremental refresh (scoped refresh)
    /// where only a single MCP server needs to be restarted.
    pub async fn start_external_server(&self, config: ExternalServerConfig) -> Result<()> {
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
                map.insert(
                    tool.name.clone(),
                    ToolLocation::External(config.name.clone()),
                );
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

    /// List all available resources from external servers
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        let mut resources = Vec::new();

        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            resources.extend(connection.list_resources().await);
        }

        resources
    }

    /// List all available prompts from external servers
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        let mut prompts = Vec::new();

        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            prompts.extend(connection.list_prompts().await);
        }

        prompts
    }

    /// Read a resource by URI
    ///
    /// The URI should include the server prefix (e.g., "server_name:file:///path")
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        let servers = self.external_servers.read().await;

        // Check if URI has server prefix
        if let Some((server_name, _resource_uri)) = uri.split_once(':') {
            // Try server with matching prefix
            if let Some(connection) = servers.get(server_name) {
                return connection.read_resource(uri).await;
            }
        }

        // Try all servers
        for connection in servers.values() {
            // Check if this server has this resource
            let resources = connection.list_resources().await;
            if resources.iter().any(|r| r.uri == uri) {
                return connection.read_resource(uri).await;
            }
        }

        Err(AlephError::NotFound(format!("Resource not found: {}", uri)))
    }

    /// Get a prompt by name with optional arguments
    ///
    /// The name should include the server prefix (e.g., "server_name:prompt_name")
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        let servers = self.external_servers.read().await;

        // Check if name has server prefix
        if let Some((server_name, _prompt_name)) = name.split_once(':') {
            // Try server with matching prefix
            if let Some(connection) = servers.get(server_name) {
                return connection.get_prompt(name, arguments).await;
            }
        }

        // Try all servers
        for connection in servers.values() {
            // Check if this server has this prompt
            let prompts = connection.list_prompts().await;
            if prompts.iter().any(|p| p.name == name) {
                return connection.get_prompt(name, arguments).await;
            }
        }

        Err(AlephError::NotFound(format!("Prompt not found: {}", name)))
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
    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<McpToolResult> {
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
        Err(AlephError::McpToolNotFound(name.to_string()))
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

    /// Start a remote MCP server connection
    ///
    /// Connects to a remote MCP server using HTTP or SSE transport.
    /// The transport is selected based on the configuration's `transport` preference.
    ///
    /// # Arguments
    ///
    /// * `config` - Remote server configuration
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the connection was established successfully
    /// * `Err(AlephError)` - If connection failed
    pub async fn start_remote_server(&self, config: McpRemoteServerConfig) -> Result<()> {
        let timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(300));

        let transport: Box<dyn McpTransport> = match config.transport {
            TransportPreference::Http => {
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via HTTP"
                );
                Box::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                ))
            }
            TransportPreference::Sse => {
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via SSE"
                );
                let transport = SseTransport::new(
                    &config.name,
                    SseTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                );

                // Set up sampling request handler for server-initiated sampling/createMessage
                let sampling_handler = Arc::clone(&self.sampling_handler);
                let server_name = config.name.clone();
                transport.set_request_handler(Box::new(move |request_id, method, params| {
                    if method == "sampling/createMessage" {
                        let handler = Arc::clone(&sampling_handler);
                        let server = server_name.clone();
                        let params_value = params.unwrap_or(serde_json::Value::Null);

                        tokio::spawn(async move {
                            tracing::debug!(
                                server = %server,
                                request_id = request_id,
                                "Processing sampling/createMessage request"
                            );

                            match handler.handle_request(request_id, params_value, &server).await {
                                Ok(response) => {
                                    tracing::debug!(
                                        server = %server,
                                        request_id = request_id,
                                        "Sampling request completed successfully"
                                    );
                                    // Note: Response sending will be handled by Task 22
                                    // For now, just log the response
                                    let _ = response;
                                }
                                Err(e) => {
                                    tracing::error!(
                                        server = %server,
                                        request_id = request_id,
                                        error = %e,
                                        "Sampling request failed"
                                    );
                                }
                            }
                        });
                    } else {
                        tracing::warn!(
                            method = %method,
                            request_id = request_id,
                            "Received unknown server-initiated request"
                        );
                    }
                }));

                // Start the SSE event listener for server-initiated notifications
                transport.start_event_listener().await?;
                Box::new(transport)
            }
            TransportPreference::Auto => {
                // Default to HTTP (most common and simpler)
                // Could add capability detection in the future
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via HTTP (auto-selected)"
                );
                Box::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        url: config.url.clone(),
                        headers: config.headers.clone(),
                        timeout,
                    },
                ))
            }
        };

        let connection = McpServerConnection::with_transport(&config.name, transport).await?;
        let connection = Arc::new(connection);

        // Register tools from this server
        let tools = connection.list_tools().await;
        {
            let mut map = self.tool_location_map.write().await;
            for tool in &tools {
                map.insert(tool.name.clone(), ToolLocation::External(config.name.clone()));
            }
        }

        tracing::info!(
            server = %config.name,
            tool_count = tools.len(),
            "Remote MCP server connected"
        );

        // Store connection
        {
            let mut servers = self.external_servers.write().await;
            servers.insert(config.name, connection);
        }

        Ok(())
    }

    /// Stop a specific external server by name
    ///
    /// Used for incremental refresh when only one server needs to be restarted.
    /// Returns true if the server was found and stopped.
    pub async fn stop_server(&self, name: &str) -> bool {
        let mut servers = self.external_servers.write().await;

        if let Some(connection) = servers.remove(name) {
            tracing::info!(server = %name, "Stopping specific MCP server");
            if let Err(e) = connection.close().await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Error stopping MCP server"
                );
            }
            true
        } else {
            tracing::debug!(server = %name, "MCP server not found (may already be stopped)");
            false
        }
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
    ///
    /// Returns the client and startup report (which contains success/failure info)
    pub async fn build_and_start(self) -> (McpClient, McpStartupReport) {
        let client = self.client;
        let report = client.start_external_servers(self.external_configs).await;
        (client, report)
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

        let result = client
            .call_tool("unknown_tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());

        match result.unwrap_err() {
            AlephError::McpToolNotFound(name) => {
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

    #[tokio::test]
    async fn test_startup_with_failing_server() {
        // Create a config with a non-existent command to simulate failure
        let failing_config = ExternalServerConfig {
            name: "failing-server".to_string(),
            command: "/nonexistent/command/that/does/not/exist".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            requires_runtime: None,
            cwd: None,
            timeout_seconds: Some(5),
        };

        let client = McpClient::new();
        let report = client.start_external_servers(vec![failing_config]).await;

        // Should have 0 succeeded and 1 failed
        assert_eq!(report.succeeded.len(), 0);
        assert_eq!(report.failed.len(), 1);

        // Check failure details
        let (server_name, error_message) = &report.failed[0];
        assert_eq!(server_name, "failing-server");
        assert!(!error_message.is_empty());
        println!("Expected failure: {} - {}", server_name, error_message);
    }

    #[tokio::test]
    async fn test_startup_report_structure() {
        // Test McpStartupReport default and methods
        let report = McpStartupReport::default();
        assert!(report.succeeded.is_empty());
        assert!(report.failed.is_empty());

        // Test with mixed results
        let mut report = McpStartupReport::default();
        report.succeeded.push("server1".to_string());
        report.succeeded.push("server2".to_string());
        report.failed.push((
            "failing-server".to_string(),
            "connection refused".to_string(),
        ));

        assert_eq!(report.succeeded.len(), 2);
        assert_eq!(report.failed.len(), 1);
    }

    #[tokio::test]
    async fn test_remote_server_config_import() {
        // Verify remote server types are accessible
        use crate::mcp::types::{McpRemoteServerConfig, TransportPreference};

        let config = McpRemoteServerConfig::new("test-remote", "https://example.com/mcp")
            .with_transport(TransportPreference::Http)
            .with_timeout(300);

        assert_eq!(config.name, "test-remote");
        assert_eq!(config.url, "https://example.com/mcp");
    }
}

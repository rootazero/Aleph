//! MCP Client - External Server Registry
//!
//! Manages external MCP server connections only.
//! Native tools (fs, git, shell, etc.) are now handled via `AgentTool` infrastructure.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_REMOTE_TIMEOUT_SECS: u64 = 300;

use crate::error::{AlephError, Result};
use crate::mcp::external::{check_runtime, McpServerConnection, RuntimeKind};
use crate::mcp::sampling::SamplingHandler;
use crate::mcp::transport::{
    HttpTransport, HttpTransportConfig, McpTransport, SseTransport, SseTransportConfig,
};
use crate::mcp::types::{
    McpRemoteServerConfig, McpTool, McpToolFilter, McpToolResult, TransportPreference,
};

/// MCP server startup report
///
/// Contains information about which servers started successfully
/// and which ones failed (with error messages).
#[derive(Debug, Clone, Default)]
pub struct McpStartupReport {
    /// Names of servers that started successfully
    pub succeeded: Vec<String>,
    /// Failed servers: (`server_name`, `error_message`)
    pub failed: Vec<(String, String)>,
}

impl McpStartupReport {
    /// Check if all servers started successfully
    #[must_use]
    pub const fn all_succeeded(&self) -> bool {
        self.failed.is_empty()
    }

    /// Get total number of servers attempted
    #[must_use]
    pub const fn total(&self) -> usize {
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

/// MCP Client - registry for external MCP server connections
///
/// Note: Native tools (fs, git, shell, etc.) are now handled via
/// the `AgentTool` infrastructure in the `tools` module. This client
/// only manages external MCP server connections.
pub struct McpClient {
    /// External server connections
    external_servers: tokio::sync::RwLock<HashMap<String, Arc<McpServerConnection>>>,
    /// Handler for sampling requests from servers
    sampling_handler: Arc<SamplingHandler>,
    /// Optional per-server allow/deny filter over advertised tools. `None`
    /// (the default) exposes every tool the connected server(s) advertise.
    /// Set once at startup via [`Self::set_tool_filter`]; applied in
    /// [`Self::list_tools`] so registration, aggregation, and counts all see
    /// the same filtered set.
    tool_filter: Option<McpToolFilter>,
}

impl McpClient {
    /// Create a new empty MCP client
    #[must_use]
    pub fn new() -> Self {
        Self {
            external_servers: tokio::sync::RwLock::new(HashMap::new()),
            sampling_handler: Arc::new(SamplingHandler::new()),
            tool_filter: None,
        }
    }

    /// Install a per-server tool filter before the client is shared.
    ///
    /// Called by the manager actor at server-start with the server's configured
    /// filter; a noop filter is normalised to `None` so [`Self::list_tools`]
    /// can skip the scan entirely. Takes `&mut self` because the filter is set
    /// once, before the client is wrapped in an `Arc` and published.
    pub fn set_tool_filter(&mut self, filter: Option<McpToolFilter>) {
        self.tool_filter = filter.filter(|f| !f.is_noop());
    }

    /// Get the sampling handler
    pub const fn sampling_handler(&self) -> &Arc<SamplingHandler> {
        &self.sampling_handler
    }

    /// Set callback for sampling requests
    pub async fn set_sampling_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(crate::mcp::jsonrpc::mcp::SamplingRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<crate::mcp::jsonrpc::mcp::SamplingResponse>>
            + Send
            + 'static,
    {
        self.sampling_handler.set_callback(callback).await;
    }

    /// Start a single external server
    ///
    /// This method is public to support incremental refresh (scoped refresh)
    /// where only a single MCP server needs to be restarted.
    ///
    /// If the config declares a required runtime (e.g. "node" for npx-based
    /// servers), the runtime is verified before spawning. A missing runtime
    /// fails fast with an actionable error instead of an opaque OS spawn
    /// failure.
    pub async fn start_external_server(&self, config: ExternalServerConfig) -> Result<()> {
        if let Some(ref runtime_str) = config.requires_runtime {
            let runtime = RuntimeKind::from_str_or_default(runtime_str);
            if runtime != RuntimeKind::None {
                let check = check_runtime(runtime);
                if !check.available {
                    tracing::warn!(
                        server = %config.name,
                        runtime = %runtime,
                        "Cannot start MCP server: {} not found",
                        runtime.display_name()
                    );
                    return Err(AlephError::NotFound(format!(
                        "MCP server '{}' requires the {} runtime, but it was not found on PATH",
                        config.name,
                        runtime.display_name()
                    )));
                }
                tracing::debug!(
                    server = %config.name,
                    runtime = %runtime,
                    version = ?check.version,
                    "Runtime check passed"
                );
            }
        }

        let timeout = config.timeout_seconds.map(Duration::from_secs);

        let connection = McpServerConnection::connect(
            &config.name,
            &config.command,
            &config.args,
            &config.env,
            config.cwd.as_ref(),
            timeout,
            Some(Arc::clone(&self.sampling_handler)),
        )
        .await?;

        let connection = Arc::new(connection);

        // Store connection
        {
            let mut servers = self.external_servers.write().await;
            servers.insert(config.name, connection);
        }

        Ok(())
    }

    /// List all available tools from external servers
    pub async fn list_tools(&self) -> Vec<McpTool> {
        // Clone Arc refs under lock, then release lock before awaiting network I/O
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            let mut conns: Vec<_> = servers.values().cloned().collect();
            conns.sort_by(|a, b| a.name().cmp(b.name()));
            conns
        };

        let mut tools = Vec::new();
        for connection in &connections {
            let mut conn_tools = connection.list_tools().await;
            // Gate advertised tools through the per-server allow/deny filter. A
            // dropped tool is therefore never registered, aggregated, counted, or
            // shown to the model (catalog-time filtering, not call-time). The
            // filter contract matches the server's *unqualified* tool names, so
            // strip the "{server}:" namespace prefix added by refresh_tools
            // before testing.
            if let Some(filter) = &self.tool_filter {
                let prefix = format!("{}:", connection.name());
                conn_tools
                    .retain(|t| filter.allows(t.name.strip_prefix(&prefix).unwrap_or(&t.name)));
            }
            tools.extend(conn_tools);
        }
        tools
    }

    /// List all available resources from external servers
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            let mut conns: Vec<_> = servers.values().cloned().collect();
            conns.sort_by(|a, b| a.name().cmp(b.name()));
            conns
        };

        let mut resources = Vec::new();
        for connection in &connections {
            resources.extend(connection.list_resources().await);
        }
        resources
    }

    /// List all resource templates from external servers
    pub async fn list_resource_templates(&self) -> Vec<crate::mcp::types::McpResourceTemplate> {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            let mut conns: Vec<_> = servers.values().cloned().collect();
            conns.sort_by(|a, b| a.name().cmp(b.name()));
            conns
        };

        let mut templates = Vec::new();
        for connection in &connections {
            templates.extend(connection.list_resource_templates().await);
        }
        templates
    }

    /// List all available prompts from external servers
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            let mut conns: Vec<_> = servers.values().cloned().collect();
            conns.sort_by(|a, b| a.name().cmp(b.name()));
            conns
        };

        let mut prompts = Vec::new();
        for connection in &connections {
            prompts.extend(connection.list_prompts().await);
        }
        prompts
    }

    /// Collect instructions from all connected MCP servers.
    pub async fn collect_instructions(
        &self,
    ) -> Vec<crate::thinker::prompt_layer::McpServerInstruction> {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            let mut conns: Vec<_> = servers.values().cloned().collect();
            conns.sort_by(|a, b| a.name().cmp(b.name()));
            conns
        };

        let mut result = Vec::new();
        for connection in &connections {
            if let Some(inst) = connection.instructions().await {
                result.push(crate::thinker::prompt_layer::McpServerInstruction {
                    server_name: connection.name().to_string(),
                    instructions: inst,
                });
            }
        }
        result
    }

    /// Find the server connection that owns `name` by looking for the
    /// longest matching "`server_name`:" prefix.  This handles server ids
    /// that themselves contain colons (e.g. "my:server:tool").
    fn find_server_by_prefix<'a>(
        &self,
        name: &str,
        servers: &'a HashMap<String, Arc<McpServerConnection>>,
    ) -> Option<&'a Arc<McpServerConnection>> {
        if !name.contains(':') {
            return None;
        }
        let mut best: Option<&Arc<McpServerConnection>> = None;
        for (id, conn) in servers {
            let prefix = format!("{id}:");
            if name.starts_with(&prefix) && best.as_ref().is_none_or(|b| b.name().len() < id.len())
            {
                best = Some(conn);
            }
        }
        best
    }

    /// Read a resource by URI
    ///
    /// The URI should include the server prefix (e.g., "`server_name:file:///path`")
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        // Clone Arc refs under lock, then release lock before awaiting network I/O
        let (direct_match, all_connections) = {
            let servers = self.external_servers.read().await;

            // Check if URI has server prefix (handles colons inside server ids)
            let direct = self.find_server_by_prefix(uri, &servers).cloned();

            let all: Vec<_> = servers.values().cloned().collect();
            (direct, all)
        };

        // Try direct match first
        if let Some(connection) = direct_match {
            return connection.read_resource(uri).await;
        }

        let mut sorted: Vec<_> = all_connections.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        for connection in sorted {
            let resources = connection.list_resources().await;
            if resources.iter().any(|r| r.uri == uri) {
                return connection.read_resource(uri).await;
            }
        }

        Err(AlephError::NotFound(format!("Resource not found: {uri}")))
    }

    /// Get a prompt by name with optional arguments
    ///
    /// The name should include the server prefix (e.g., "`server_name:prompt_name`")
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        // Clone Arc refs under lock, then release lock before awaiting network I/O
        let (direct_match, all_connections) = {
            let servers = self.external_servers.read().await;

            let direct = self.find_server_by_prefix(name, &servers).cloned();

            let all: Vec<_> = servers.values().cloned().collect();
            (direct, all)
        };

        if let Some(connection) = direct_match {
            return connection.get_prompt(name, arguments).await;
        }

        let mut sorted: Vec<_> = all_connections.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        let mut matching = None;
        for connection in sorted {
            let prompts = connection.list_prompts().await;
            if prompts.iter().any(|p| p.name == name) {
                matching = Some(connection);
                break;
            }
        }
        if let Some(connection) = matching {
            return connection.get_prompt(name, arguments).await;
        }

        Err(AlephError::NotFound(format!("Prompt not found: {name}")))
    }

    /// Get tools as a formatted list for context injection
    pub async fn get_tools_for_context(&self) -> Vec<(String, String, serde_json::Value)> {
        self.list_tools()
            .await
            .into_iter()
            .map(|t| (t.name, t.description, t.input_schema))
            .collect()
    }

    /// Call a tool by name
    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<McpToolResult> {
        // Clone Arc refs under lock, then release lock before awaiting network I/O
        let (direct_match, all_connections) = {
            let servers = self.external_servers.read().await;

            let direct = self.find_server_by_prefix(name, &servers).cloned();

            let all: Vec<_> = servers.values().cloned().collect();
            (direct, all)
        };

        if let Some(connection) = direct_match {
            let result = connection.call_tool(name, args).await?;
            return Ok(McpToolResult::success(result));
        }

        let mut sorted: Vec<_> = all_connections.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        for connection in sorted {
            if connection.has_tool(name).await {
                let result = connection.call_tool(name, args).await?;
                return Ok(McpToolResult::success(result));
            }
        }

        Err(AlephError::McpToolNotFound(name.to_string()))
    }

    /// Get list of registered external server names
    pub async fn service_names(&self) -> Vec<String> {
        let servers = self.external_servers.read().await;
        let mut names: Vec<String> = servers.keys().cloned().collect();
        names.sort();
        names
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
        // OAuth wiring: when no explicit Authorization header is configured,
        // fall back to tokens stored by the `mcp_login` flow (refreshing
        // expired ones). Injected before preflight so auth-gated endpoints
        // are probed with credentials.
        let mut config = config;
        if !config.headers.contains_key("Authorization") {
            if let Some(token) =
                crate::mcp::auth::stored_bearer_token(&config.name, &config.url).await
            {
                tracing::info!(
                    server = %config.name,
                    "Using stored OAuth token for remote MCP server"
                );
                config
                    .headers
                    .insert("Authorization".to_string(), format!("Bearer {token}"));
            }
        }

        // Fail fast if the URL clearly serves a web page rather than an MCP
        // endpoint, instead of burning the full connect timeout on a doomed
        // handshake. Lenient: only an unambiguous HTML response is rejected.
        crate::mcp::preflight_remote_url(&config.url, &config.headers).await?;

        let timeout = Duration::from_secs(
            config
                .timeout_seconds
                .unwrap_or(DEFAULT_REMOTE_TIMEOUT_SECS),
        );

        let transport: Arc<dyn McpTransport> = match config.transport {
            TransportPreference::Http => {
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via HTTP"
                );
                Arc::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        // rust-doctor-disable-next-line excessive-clone
                        url: config.url.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        headers: config.headers.clone(),
                        timeout,
                    },
                )?)
            }
            TransportPreference::Sse => {
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via SSE"
                );
                let transport = Arc::new(SseTransport::new(
                    &config.name,
                    SseTransportConfig {
                        // rust-doctor-disable-next-line excessive-clone
                        url: config.url.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        headers: config.headers.clone(),
                        timeout,
                    },
                )?);

                // Set up sampling request handler for server-initiated sampling/createMessage
                let sampling_handler = Arc::clone(&self.sampling_handler);
                // rust-doctor-disable-next-line excessive-clone
                let server_name = config.name.clone();
                let transport_for_handler = Arc::clone(&transport);
                transport.set_request_handler(Box::new(move |request_id, method, params| {
                    if method == "sampling/createMessage" {
                        let handler = Arc::clone(&sampling_handler);
                        // rust-doctor-disable-next-line excessive-clone
                        let server = server_name.clone();
                        let params_value = params.unwrap_or(serde_json::Value::Null);
                        // rust-doctor-disable-next-line excessive-clone
                        let rid = request_id.clone();
                        let transport = Arc::clone(&transport_for_handler);

                        tokio::spawn(async move {
                            tracing::debug!(
                                server = %server,
                                request_id = %rid,
                                "Processing sampling/createMessage request"
                            );

                            match handler
                                // rust-doctor-disable-next-line excessive-clone
                                .handle_request(rid.clone(), params_value, &server)
                                .await
                            {
                                Ok(response) => {
                                    tracing::debug!(
                                        server = %server,
                                        request_id = %rid,
                                        "Sampling request completed successfully"
                                    );
                                    match serde_json::to_value(&response) {
                                        Ok(result) => {
                                            if let Err(e) = transport
                                                // rust-doctor-disable-next-line excessive-clone
                                                .send_sampling_response(rid.clone(), result)
                                                .await
                                            {
                                                tracing::error!(
                                                    server = %server,
                                                    request_id = %rid,
                                                    error = %e,
                                                    "Failed to send sampling response"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                server = %server,
                                                request_id = %rid,
                                                error = %e,
                                                "Failed to serialize sampling response"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        server = %server,
                                        request_id = %rid,
                                        error = %e,
                                        "Sampling request failed"
                                    );
                                }
                            }
                        });
                    } else {
                        tracing::warn!(
                            method = %method,
                            request_id = %request_id,
                            "Received unknown server-initiated request"
                        );
                    }
                }));

                // Start the SSE event listener for server-initiated notifications
                transport.start_event_listener().await?;
                transport
            }
            TransportPreference::Auto => {
                // Default to HTTP (most common and simpler)
                // Could add capability detection in the future
                tracing::info!(
                    server = %config.name,
                    url = %config.url,
                    "Connecting to remote MCP server via HTTP (auto-selected)"
                );
                Arc::new(HttpTransport::new(
                    &config.name,
                    HttpTransportConfig {
                        // rust-doctor-disable-next-line excessive-clone
                        url: config.url.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        headers: config.headers.clone(),
                        timeout,
                    },
                )?)
            }
        };

        let connection = McpServerConnection::with_transport(
            &config.name,
            transport,
            Some(Arc::clone(&self.sampling_handler)),
        )
        .await?;
        let connection = Arc::new(connection);

        let tool_count = connection.list_tools().await.len();
        tracing::info!(
            server = %config.name,
            tool_count,
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
        // Clone Arc refs under lock, then release lock before awaiting network I/O
        let connections: Vec<(String, Arc<McpServerConnection>)> = {
            let servers = self.external_servers.read().await;
            servers
                .iter()
                // rust-doctor-disable-next-line excessive-clone
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        // Probe every server concurrently: transport-level liveness AND an
        // active RPC `ping`. The ping is the only real reachability signal for
        // a stateless HTTP transport (whose is_running is always true) and a
        // keepalive for long-lived ones. Running the probes concurrently bounds
        // the whole pass to the slowest single probe instead of their sum.
        let probes = connections
            .into_iter()
            .map(|(name, connection)| async move {
                let healthy = connection.is_running().await && connection.ping().await;
                (name, healthy)
            });

        futures::future::join_all(probes)
            .await
            .into_iter()
            .collect()
    }

    /// Install a notification handler on a specific server's connection.
    ///
    /// Used by the manager to route `*/list_changed` notifications back into
    /// the actor so the tool registry and capability state stay in sync.
    pub async fn set_notification_handler(
        &self,
        server: &str,
        handler: crate::mcp::transport::NotificationCallback,
    ) {
        let servers = self.external_servers.read().await;
        match servers.get(server) {
            Some(connection) => connection.set_notification_handler(handler),
            None => tracing::warn!(
                server = %server,
                "set_notification_handler: no such connected MCP server"
            ),
        }
    }

    /// Re-fetch tool/resource/prompt caches for every connection.
    ///
    /// Called when a server announces a list-changed notification so that a
    /// subsequent `list_tools` / `list_resources` / `list_prompts` reflects
    /// the server's current state.
    pub async fn refresh_caches(&self) {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            servers.values().cloned().collect()
        };
        for connection in &connections {
            if let Err(e) = connection.refresh_tools().await {
                tracing::warn!(server = %connection.name(), error = %e, "MCP refresh tools failed");
            }
            if let Err(e) = connection.refresh_resources().await {
                tracing::debug!(server = %connection.name(), error = %e, "MCP refresh resources failed");
            }
            if let Err(e) = connection.refresh_resource_templates().await {
                tracing::debug!(server = %connection.name(), error = %e, "MCP refresh resource templates failed");
            }
            if let Err(e) = connection.refresh_prompts().await {
                tracing::debug!(server = %connection.name(), error = %e, "MCP refresh prompts failed");
            }
        }
    }

    /// Re-fetch every cached list whose server-supplied `ttlMs` has lapsed, and
    /// report which kinds actually changed.
    ///
    /// Driven by the manager's health tick. See
    /// [`McpServerConnection::refresh_expired_lists`] for why expiry alone is
    /// not treated as a change.
    pub async fn refresh_expired_lists(&self) -> crate::mcp::external::ChangedLists {
        let connections: Vec<_> = {
            let servers = self.external_servers.read().await;
            servers.values().cloned().collect()
        };

        let mut changed = crate::mcp::external::ChangedLists::default();
        for connection in &connections {
            changed = changed.merged(connection.refresh_expired_lists().await);
        }
        changed
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating `McpClient` with configuration
pub struct McpClientBuilder {
    client: McpClient,
}

impl McpClientBuilder {
    /// Create a new builder
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: McpClient::new(),
        }
    }

    /// Build the client (without starting external servers)
    pub fn build(self) -> McpClient {
        self.client
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

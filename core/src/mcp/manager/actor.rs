//! MCP Manager Actor
//!
//! The actor that handles command processing, server lifecycle,
//! config persistence, and event broadcasting.
//!
//! # Architecture
//!
//! The actor follows a message-passing pattern:
//! - Commands arrive via `mpsc` channel from `McpManagerHandle`
//! - Events are broadcast to subscribers via `broadcast` channel
//! - Configuration is persisted to JSON file on disk
//!
//! # Lifecycle
//!
//! 1. `McpManagerActor::new()` - Load config, create channels, return handle
//! 2. `McpManagerActor::run()` - Main loop processing commands
//! 3. Shutdown - Stop all servers, broadcast event, exit

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

use super::config::McpPersistentConfig;
use super::handle::McpManagerHandle;
use super::types::{
    HealthStatus, McpCommand, McpManagerConfig, McpManagerEvent, McpServerInfo,
    McpServerStatusDetail, McpTransportType, ServerHealth,
};
use crate::mcp::{
    ExternalServerConfig, McpClient, McpPrompt, McpRemoteServerConfig, McpResource, McpTool,
    TransportPreference,
};

/// Configuration for health check behavior
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Interval between health checks
    pub interval: Duration,
    /// Timeout for health check request
    pub timeout: Duration,
    /// Number of failures before marking unhealthy
    pub max_failures: u32,
    /// Delay before restart attempt
    pub restart_delay: Duration,
    /// Maximum restart attempts in window
    pub max_restarts: u32,
    /// Duration of restart window
    pub restart_window: Duration,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            max_failures: 3,
            restart_delay: Duration::from_secs(2),
            max_restarts: 3,
            restart_window: Duration::from_secs(300),
        }
    }
}

/// The MCP Manager Actor
///
/// Orchestrates multiple MCP server connections, handling lifecycle management,
/// health monitoring, and capability aggregation.
pub struct McpManagerActor {
    /// Path to the configuration file
    config_path: PathBuf,
    /// Loaded configuration
    config: McpPersistentConfig,
    /// Active MCP clients by server ID
    clients: HashMap<String, Arc<McpClient>>,
    /// Health tracking per server
    health_states: HashMap<String, ServerHealth>,
    /// Server start times for uptime tracking
    start_times: HashMap<String, Instant>,
    /// Health check configuration
    health_config: HealthCheckConfig,
    /// Event broadcaster
    event_tx: broadcast::Sender<McpManagerEvent>,
    /// Command receiver
    cmd_rx: mpsc::Receiver<McpCommand>,
    /// Command sender (for handle creation)
    cmd_tx: mpsc::Sender<McpCommand>,
}

impl McpManagerActor {
    /// Create a new MCP Manager Actor
    ///
    /// Loads configuration from the specified path (or default path),
    /// expands environment variables, and creates communication channels.
    ///
    /// # Arguments
    ///
    /// * `config_path` - Optional path to config file, defaults to `~/.aether/mcp_config.json`
    ///
    /// # Returns
    ///
    /// A tuple of the actor and its handle for public API access
    pub async fn new(
        config_path: Option<PathBuf>,
    ) -> Result<(Self, McpManagerHandle), String> {
        let config_path = config_path.unwrap_or_else(McpPersistentConfig::default_path);

        // Load and expand configuration
        let mut config = McpPersistentConfig::load(&config_path)
            .await
            .map_err(|e| format!("Failed to load MCP config: {}", e))?;
        config.expand_env_vars();

        // Create channels
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        // Create handle
        let handle = McpManagerHandle::new(cmd_tx.clone(), event_tx.clone());

        let actor = Self {
            config_path,
            config,
            clients: HashMap::new(),
            health_states: HashMap::new(),
            start_times: HashMap::new(),
            health_config: HealthCheckConfig::default(),
            event_tx,
            cmd_rx,
            cmd_tx,
        };

        Ok((actor, handle))
    }

    /// Get a handle to this actor
    ///
    /// Creates a new handle that can be used to send commands.
    pub fn handle(&self) -> McpManagerHandle {
        McpManagerHandle::new(self.cmd_tx.clone(), self.event_tx.clone())
    }

    /// Run the actor's main loop
    ///
    /// This method:
    /// 1. Auto-starts servers from configuration
    /// 2. Broadcasts `ManagerReady` event
    /// 3. Processes commands until shutdown
    /// 4. Shuts down all servers gracefully
    pub async fn run(mut self) {
        tracing::info!("MCP Manager starting...");

        // Auto-start servers from config
        let auto_start_configs: Vec<_> = self
            .config
            .auto_start_servers()
            .iter()
            .map(|c| (*c).clone())
            .collect();

        for config in auto_start_configs {
            if let Err(e) = self.start_server_internal(&config).await {
                tracing::error!(
                    server_id = %config.id,
                    error = %e,
                    "Failed to auto-start server"
                );
            }
        }

        // Broadcast ready event
        let _ = self.event_tx.send(McpManagerEvent::ManagerReady);
        tracing::info!("MCP Manager ready with {} servers", self.clients.len());

        // Main command loop
        while let Some(cmd) = self.cmd_rx.recv().await {
            if !self.handle_command(cmd).await {
                break;
            }
        }

        // Shutdown sequence
        tracing::info!("MCP Manager shutting down...");
        self.shutdown_all().await;
        let _ = self.event_tx.send(McpManagerEvent::ManagerShutdown);
        tracing::info!("MCP Manager shutdown complete");
    }

    /// Handle a single command
    ///
    /// Returns `false` if the actor should shutdown.
    async fn handle_command(&mut self, cmd: McpCommand) -> bool {
        match cmd {
            McpCommand::AddServer { config, respond_to } => {
                let result = self.add_server(config).await;
                let _ = respond_to.send(result);
            }
            McpCommand::RemoveServer {
                server_id,
                respond_to,
            } => {
                let result = self.remove_server(&server_id).await;
                let _ = respond_to.send(result);
            }
            McpCommand::RestartServer {
                server_id,
                respond_to,
            } => {
                let result = self.restart_server(&server_id).await;
                let _ = respond_to.send(result);
            }
            McpCommand::StartServer {
                server_id,
                respond_to,
            } => {
                let result = self.start_server(&server_id).await;
                let _ = respond_to.send(result);
            }
            McpCommand::StopServer {
                server_id,
                respond_to,
            } => {
                let result = self.stop_server(&server_id).await;
                let _ = respond_to.send(result);
            }
            McpCommand::GetClient {
                server_id,
                respond_to,
            } => {
                let client = self.clients.get(&server_id).cloned();
                let _ = respond_to.send(client);
            }
            McpCommand::ListServers { respond_to } => {
                let servers = self.list_servers().await;
                let _ = respond_to.send(servers);
            }
            McpCommand::GetStatus {
                server_id,
                respond_to,
            } => {
                let status = self.get_status(&server_id).await;
                let _ = respond_to.send(status);
            }
            McpCommand::AggregateTools { respond_to } => {
                let tools = self.aggregate_tools().await;
                let _ = respond_to.send(tools);
            }
            McpCommand::AggregateResources { respond_to } => {
                let resources = self.aggregate_resources().await;
                let _ = respond_to.send(resources);
            }
            McpCommand::AggregatePrompts { respond_to } => {
                let prompts = self.aggregate_prompts().await;
                let _ = respond_to.send(prompts);
            }
            McpCommand::ReloadConfig { respond_to } => {
                let result = self.reload_config().await;
                let _ = respond_to.send(result);
            }
            McpCommand::Shutdown { respond_to } => {
                let _ = respond_to.send(());
                return false;
            }
        }
        true
    }

    // ===== Lifecycle Methods =====

    /// Add a server configuration
    ///
    /// Upserts the config, saves to disk, and optionally starts if auto_start is true.
    async fn add_server(&mut self, config: McpManagerConfig) -> Result<(), String> {
        let server_id = config.id.clone();
        let server_name = config.name.clone();
        let auto_start = config.auto_start;

        // Upsert config
        self.config.upsert_server(config.clone());

        // Save to disk
        self.config
            .save(&self.config_path)
            .await
            .map_err(|e| format!("Failed to save config: {}", e))?;

        // Broadcast event
        let _ = self.event_tx.send(McpManagerEvent::ServerAdded {
            server_id: server_id.clone(),
            server_name: server_name.clone(),
        });

        // Start if auto_start
        if auto_start {
            self.start_server_internal(&config).await?;
        }

        tracing::info!(server_id = %server_id, "Server added");
        Ok(())
    }

    /// Remove a server
    ///
    /// Stops the server if running, removes from config, saves to disk.
    async fn remove_server(&mut self, server_id: &str) -> Result<(), String> {
        // Get server name before removal for event
        let server_name = self
            .config
            .get_server(server_id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| server_id.to_string());

        // Stop if running
        self.stop_server_internal(server_id).await;

        // Remove from config
        self.config.remove_server(server_id);

        // Save to disk
        self.config
            .save(&self.config_path)
            .await
            .map_err(|e| format!("Failed to save config: {}", e))?;

        // Broadcast event
        let _ = self.event_tx.send(McpManagerEvent::ServerRemoved {
            server_id: server_id.to_string(),
            server_name,
        });

        tracing::info!(server_id = %server_id, "Server removed");
        Ok(())
    }

    /// Restart a server
    ///
    /// Stops, waits, then starts the server again.
    async fn restart_server(&mut self, server_id: &str) -> Result<(), String> {
        let config = self
            .config
            .get_server(server_id)
            .cloned()
            .ok_or_else(|| format!("Server not found: {}", server_id))?;

        let server_name = config.name.clone();

        // Update health state
        if let Some(health) = self.health_states.get_mut(server_id) {
            health.mark_restarting();
        }

        // Broadcast restarting event
        let attempt = self
            .health_states
            .get(server_id)
            .map(|h| h.restart_count)
            .unwrap_or(1);
        let _ = self.event_tx.send(McpManagerEvent::ServerRestarting {
            server_id: server_id.to_string(),
            server_name,
            attempt,
        });

        // Stop the server
        self.stop_server_internal(server_id).await;

        // Wait before restarting
        tokio::time::sleep(self.health_config.restart_delay).await;

        // Start the server
        self.start_server_internal(&config).await?;

        tracing::info!(server_id = %server_id, "Server restarted");
        Ok(())
    }

    /// Start a stopped server
    async fn start_server(&mut self, server_id: &str) -> Result<(), String> {
        // Check if already running
        if self.clients.contains_key(server_id) {
            return Err(format!("Server already running: {}", server_id));
        }

        let config = self
            .config
            .get_server(server_id)
            .cloned()
            .ok_or_else(|| format!("Server not found: {}", server_id))?;

        self.start_server_internal(&config).await
    }

    /// Stop a running server
    async fn stop_server(&mut self, server_id: &str) -> Result<(), String> {
        if !self.clients.contains_key(server_id) {
            return Err(format!("Server not running: {}", server_id));
        }

        let server_name = self
            .config
            .get_server(server_id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| server_id.to_string());

        self.stop_server_internal(server_id).await;

        // Broadcast stopped event
        let _ = self.event_tx.send(McpManagerEvent::ServerStopped {
            server_id: server_id.to_string(),
            server_name,
        });

        tracing::info!(server_id = %server_id, "Server stopped");
        Ok(())
    }

    /// Internal method to start a server
    ///
    /// Creates an McpClient and connects using the appropriate transport.
    async fn start_server_internal(&mut self, config: &McpManagerConfig) -> Result<(), String> {
        let client = Arc::new(McpClient::new());

        // Start based on transport type
        match config.transport {
            McpTransportType::Stdio => {
                let command = config
                    .command
                    .as_ref()
                    .ok_or_else(|| format!("No command specified for stdio server: {}", config.id))?;

                let external_config = ExternalServerConfig {
                    name: config.id.clone(),
                    command: command.clone(),
                    args: config.args.clone(),
                    env: config.env.clone(),
                    cwd: None,
                    requires_runtime: config.requires_runtime.clone(),
                    timeout_seconds: config.timeout_seconds,
                };

                client
                    .start_external_server(external_config)
                    .await
                    .map_err(|e| format!("Failed to start stdio server: {}", e))?;
            }
            McpTransportType::Http | McpTransportType::Sse => {
                let url = config
                    .url
                    .as_ref()
                    .ok_or_else(|| format!("No URL specified for remote server: {}", config.id))?;

                let transport = match config.transport {
                    McpTransportType::Http => TransportPreference::Http,
                    McpTransportType::Sse => TransportPreference::Sse,
                    _ => TransportPreference::Auto,
                };

                let remote_config = McpRemoteServerConfig::new(&config.id, url)
                    .with_transport(transport);

                let remote_config = if let Some(timeout) = config.timeout_seconds {
                    remote_config.with_timeout(timeout)
                } else {
                    remote_config
                };

                client
                    .start_remote_server(remote_config)
                    .await
                    .map_err(|e| format!("Failed to start remote server: {}", e))?;
            }
        }

        // Get tool count for event
        let tool_count = client.list_tools().await.len();

        // Store client and track start time
        self.clients.insert(config.id.clone(), client);
        self.start_times.insert(config.id.clone(), Instant::now());
        self.health_states
            .insert(config.id.clone(), ServerHealth::healthy());

        // Broadcast started event
        let _ = self.event_tx.send(McpManagerEvent::ServerStarted {
            server_id: config.id.clone(),
            server_name: config.name.clone(),
            tool_count,
        });

        tracing::info!(
            server_id = %config.id,
            tool_count = tool_count,
            "Server started"
        );

        Ok(())
    }

    /// Internal method to stop a server
    ///
    /// Calls client.stop_all() and removes from tracking maps.
    async fn stop_server_internal(&mut self, server_id: &str) {
        if let Some(client) = self.clients.remove(server_id) {
            if let Err(e) = client.stop_all().await {
                tracing::warn!(
                    server_id = %server_id,
                    error = %e,
                    "Error stopping server"
                );
            }
        }

        self.start_times.remove(server_id);

        // Update health state to stopped
        if let Some(health) = self.health_states.get_mut(server_id) {
            health.mark_stopped();
        }
    }

    /// Shutdown all servers
    async fn shutdown_all(&mut self) {
        let server_ids: Vec<_> = self.clients.keys().cloned().collect();
        for server_id in server_ids {
            self.stop_server_internal(&server_id).await;
        }
    }

    // ===== Query Methods =====

    /// List all servers with their status
    async fn list_servers(&self) -> Vec<McpServerInfo> {
        let mut servers = Vec::new();

        for (id, config) in &self.config.servers {
            let health = self
                .health_states
                .get(id)
                .map(|h| h.status.clone())
                .unwrap_or(HealthStatus::Stopped);

            // Get tool/resource/prompt counts from active clients
            let (tool_count, resource_count, prompt_count) = if let Some(client) = self.clients.get(id) {
                let tools = client.list_tools().await.len();
                let resources = client.list_resources().await.len();
                let prompts = client.list_prompts().await.len();
                (tools, resources, prompts)
            } else {
                (0, 0, 0)
            };

            servers.push(McpServerInfo {
                id: id.clone(),
                name: config.name.clone(),
                transport: config.transport,
                tool_count,
                resource_count,
                prompt_count,
                health,
            });
        }

        servers
    }

    /// Get detailed status for a specific server
    async fn get_status(&self, server_id: &str) -> Option<McpServerStatusDetail> {
        let config = self.config.get_server(server_id)?;

        let health = self
            .health_states
            .get(server_id)
            .cloned()
            .unwrap_or_default();

        let (tools, resources, prompts) = if let Some(client) = self.clients.get(server_id) {
            let tools = client.list_tools().await;
            let resources = client.list_resources().await;
            let prompts = client.list_prompts().await;
            (tools, resources, prompts)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        Some(McpServerStatusDetail {
            id: server_id.to_string(),
            name: config.name.clone(),
            transport: config.transport,
            health,
            tools,
            resources,
            prompts,
            config: config.clone(),
        })
    }

    // ===== Aggregation Methods =====

    /// Aggregate tools from all healthy servers
    async fn aggregate_tools(&self) -> Vec<McpTool> {
        let mut all_tools = Vec::new();

        for (server_id, client) in &self.clients {
            // Check health - only aggregate from healthy servers
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded { .. }) {
                    continue;
                }
            }

            let tools = client.list_tools().await;
            all_tools.extend(tools);
        }

        all_tools
    }

    /// Aggregate resources from all healthy servers
    async fn aggregate_resources(&self) -> Vec<McpResource> {
        let mut all_resources = Vec::new();

        for (server_id, client) in &self.clients {
            // Check health - only aggregate from healthy servers
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded { .. }) {
                    continue;
                }
            }

            let resources = client.list_resources().await;
            all_resources.extend(resources);
        }

        all_resources
    }

    /// Aggregate prompts from all healthy servers
    async fn aggregate_prompts(&self) -> Vec<McpPrompt> {
        let mut all_prompts = Vec::new();

        for (server_id, client) in &self.clients {
            // Check health - only aggregate from healthy servers
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded { .. }) {
                    continue;
                }
            }

            let prompts = client.list_prompts().await;
            all_prompts.extend(prompts);
        }

        all_prompts
    }

    // ===== Config Methods =====

    /// Reload configuration from disk
    ///
    /// Reconciles running state with new configuration:
    /// - Stops servers that were removed
    /// - Starts new servers with auto_start=true
    /// - Updates configs for existing servers
    async fn reload_config(&mut self) -> Result<(), String> {
        // Load new config
        let mut new_config = McpPersistentConfig::load(&self.config_path)
            .await
            .map_err(|e| format!("Failed to reload config: {}", e))?;
        new_config.expand_env_vars();

        // Find servers to stop (in old config but not in new)
        let servers_to_stop: Vec<_> = self
            .config
            .servers
            .keys()
            .filter(|id| !new_config.servers.contains_key(*id))
            .cloned()
            .collect();

        // Stop removed servers
        for server_id in servers_to_stop {
            self.stop_server_internal(&server_id).await;
            tracing::info!(server_id = %server_id, "Stopped removed server");
        }

        // Find new servers to start
        let servers_to_start: Vec<_> = new_config
            .servers
            .values()
            .filter(|config| {
                config.auto_start && !self.config.servers.contains_key(&config.id)
            })
            .cloned()
            .collect();

        // Update config
        self.config = new_config;

        // Start new auto-start servers
        for config in servers_to_start {
            if let Err(e) = self.start_server_internal(&config).await {
                tracing::error!(
                    server_id = %config.id,
                    error = %e,
                    "Failed to start new server from reloaded config"
                );
            }
        }

        // Broadcast event
        let _ = self.event_tx.send(McpManagerEvent::ConfigReloaded {
            server_count: self.config.servers.len(),
        });

        tracing::info!("Configuration reloaded");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_actor_creation() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let result = McpManagerActor::new(Some(config_path)).await;
        assert!(result.is_ok());

        let (actor, _handle) = result.unwrap();
        assert!(actor.clients.is_empty());
        assert!(actor.health_states.is_empty());
    }

    #[tokio::test]
    async fn test_actor_with_default_path_stub() {
        // This test just verifies the structure compiles correctly
        // Actual default path creation would require filesystem access
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("test_config.json");

        let (actor, handle) = McpManagerActor::new(Some(config_path)).await.unwrap();

        assert!(handle.is_running());
        assert!(actor.config.servers.is_empty());
    }

    #[test]
    fn test_health_check_config_default() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert_eq!(config.max_failures, 3);
        assert_eq!(config.restart_delay, Duration::from_secs(2));
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.restart_window, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_list_servers_empty() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let servers = actor.list_servers().await;
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn test_get_status_nonexistent() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let status = actor.get_status("nonexistent").await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_aggregate_tools_empty() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let tools = actor.aggregate_tools().await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_aggregate_resources_empty() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let resources = actor.aggregate_resources().await;
        assert!(resources.is_empty());
    }

    #[tokio::test]
    async fn test_aggregate_prompts_empty() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let prompts = actor.aggregate_prompts().await;
        assert!(prompts.is_empty());
    }

    #[tokio::test]
    async fn test_handle_creation() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");

        let (actor, handle1) = McpManagerActor::new(Some(config_path)).await.unwrap();
        let handle2 = actor.handle();

        assert!(handle1.is_running());
        assert!(handle2.is_running());
    }
}

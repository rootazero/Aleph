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

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use super::config::McpPersistentConfig;
use super::handle::McpManagerHandle;
use super::types::{
    HealthStatus, ListChangeKind, McpCommand, McpManagerConfig, McpManagerEvent, McpServerInfo,
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
    /// Consecutive failures before a server is marked unhealthy
    /// (consumed by `ServerHealth::record_failure`).
    pub max_failures: u32,
    /// Maximum restart attempts in window
    pub max_restarts: u32,
    /// Duration of restart window
    pub restart_window: Duration,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            max_failures: 3,
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
    /// Health check configuration
    health_config: HealthCheckConfig,
    /// Event broadcaster
    event_tx: broadcast::Sender<McpManagerEvent>,
    /// Command receiver
    cmd_rx: mpsc::Receiver<McpCommand>,
    /// Command sender (for handle creation)
    cmd_tx: mpsc::Sender<McpCommand>,
    /// Stored sampling callback for new servers
    sampling_callback: Option<Arc<crate::mcp::sampling::SamplingCallback>>,
    /// Optional resolver for `{{secret:NAME}}` env references, applied
    /// per-server at spawn so secrets reach only that child's environment.
    secret_resolver: Option<Arc<dyn crate::secrets::AsyncSecretResolver>>,
}

impl McpManagerActor {
    /// Create a new MCP Manager Actor
    ///
    /// Loads configuration from the specified path (or default path),
    /// expands environment variables, and creates communication channels.
    ///
    /// # Arguments
    ///
    /// * `config_path` - Optional path to config file, defaults to `~/.aleph/mcp_config.json`
    ///
    /// # Returns
    ///
    /// A tuple of the actor and its handle for public API access
    pub async fn new(config_path: Option<PathBuf>) -> Result<(Self, McpManagerHandle), String> {
        let config_path = config_path.unwrap_or_else(McpPersistentConfig::default_path);

        // Load and expand configuration
        let mut config = McpPersistentConfig::load(&config_path)
            .await
            .map_err(|e| format!("Failed to load MCP config: {e}"))?;
        config.expand_env_vars();

        // Create channels
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (event_tx, _) = broadcast::channel(64);

        // Create handle
        // rust-doctor-disable-next-line excessive-clone
        let handle = McpManagerHandle::new(cmd_tx.clone(), event_tx.clone());

        let actor = Self {
            config_path,
            config,
            clients: HashMap::new(),
            health_states: HashMap::new(),
            health_config: HealthCheckConfig::default(),
            event_tx,
            cmd_rx,
            cmd_tx,
            sampling_callback: None,
            secret_resolver: None,
        };

        Ok((actor, handle))
    }

    /// Get a handle to this actor
    ///
    /// Creates a new handle that can be used to send commands.
    #[must_use]
    pub fn handle(&self) -> McpManagerHandle {
        // rust-doctor-disable-next-line excessive-clone
        McpManagerHandle::new(self.cmd_tx.clone(), self.event_tx.clone())
    }

    /// Install a secret resolver used to resolve `{{secret:NAME}}` env
    /// references into each MCP child's environment at spawn time.
    ///
    /// Must be set before `run()` so persisted vault-backed servers that
    /// auto-start at boot can resolve their secrets.
    #[must_use]
    pub fn with_secret_resolver(
        mut self,
        resolver: Arc<dyn crate::secrets::AsyncSecretResolver>,
    ) -> Self {
        self.secret_resolver = Some(resolver);
        self
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
        let auto_start_configs: Vec<McpManagerConfig> = self
            .config
            .auto_start_servers()
            .into_iter()
            .cloned()
            .collect();

        for config in &auto_start_configs {
            if let Err(e) = self.start_server_internal(config).await {
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

        // Main command loop, interleaved with periodic health checks. The
        // health tick fires once immediately; that first tick is consumed
        // before the loop because servers were just auto-started above.
        let mut shutdown_respond_to = None;
        let mut health_tick = tokio::time::interval(self.health_config.interval);
        health_tick.tick().await;
        loop {
            tokio::select! {
                maybe_cmd = self.cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(McpCommand::Shutdown { respond_to }) => {
                            shutdown_respond_to = Some(respond_to);
                            break;
                        }
                        Some(other) => {
                            if !self.handle_command(other).await {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = health_tick.tick() => {
                    self.health_check_pass().await;
                }
            }
        }

        // Shutdown sequence — ACK only after all servers are actually stopped
        tracing::info!("MCP Manager shutting down...");
        self.shutdown_all().await;
        if let Some(respond_to) = shutdown_respond_to {
            let _ = respond_to.send(());
        }
        let _ = self.event_tx.send(McpManagerEvent::ManagerShutdown);
        tracing::info!("MCP Manager shutdown complete");
    }

    /// Periodic health probe over every running server.
    ///
    /// For each server it checks transport liveness, drives the per-server
    /// circuit breaker (`ServerHealth`), and auto-restarts servers that have
    /// failed past the unhealthy threshold — subject to the restart-window
    /// cap so a permanently-broken server does not restart-loop forever.
    ///
    /// The probe doubles as a keepalive: for stdio it confirms the child
    /// process is alive, for SSE that the event stream is still connected.
    /// HTTP transports are stateless, so their probe is always healthy and a
    /// dead endpoint instead surfaces as ordinary tool-call failures.
    async fn health_check_pass(&mut self) {
        // Snapshot (id, client) up front: the probe awaits, and we must not
        // hold a borrow of `self.clients` across the `restart_server` call.
        let probes: Vec<(String, Arc<McpClient>)> = self
            .clients
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|(id, c)| (id.clone(), Arc::clone(c)))
            .collect();
        if probes.is_empty() {
            return;
        }

        let mut to_restart: Vec<String> = Vec::new();
        for (server_id, client) in probes {
            let alive = client.check_server_health().await.values().all(|&ok| ok);
            if alive {
                if let Some(health) = self.health_states.get_mut(&server_id) {
                    health.record_success();
                }

                // Revision 2026-07-28 dropped the always-on server-to-client
                // stream, so for a client that has not opened a
                // `subscriptions/listen` stream a lapsed cache TTL is the only
                // signal that a list may have moved. Anything that really did
                // change is fed into the same path a server-sent notification
                // takes, so there is one publisher rather than two.
                for kind in changed_list_kinds(client.refresh_expired_lists().await) {
                    let _ = self.cmd_tx.try_send(McpCommand::ServerListChanged {
                        // rust-doctor-disable-next-line excessive-clone
                        server_id: server_id.clone(),
                        kind,
                    });
                }
            } else if let Some(health) = self.health_states.get_mut(&server_id) {
                health.record_failure(
                    "health probe: transport not alive",
                    self.health_config.max_failures,
                );
                if health.should_restart(
                    self.health_config.max_restarts,
                    self.health_config.restart_window.as_secs(),
                ) {
                    to_restart.push(server_id);
                }
            }
        }

        for server_id in to_restart {
            let server_name = self
                .config
                .get_server(&server_id)
                .map(|c| c.name.as_str())
                .unwrap_or(server_id.as_str());
            // Emit ServerCrashed first so the tool bridge drops the dead
            // server's tools from the registry before the restart re-publishes
            // a fresh set via the ServerStarted event.
            let _ = self.event_tx.send(McpManagerEvent::ServerCrashed {
                // rust-doctor-disable-next-line excessive-clone
                server_id: server_id.clone(),
                server_name: server_name.to_string(),
                error: "health probe failed; auto-restarting".to_string(),
            });
            match self.restart_server(&server_id).await {
                Ok(()) => {
                    tracing::info!(server_id = %server_id, "auto-restarted unhealthy MCP server");
                }
                Err(e) => {
                    tracing::warn!(server_id = %server_id, error = %e, "auto-restart failed");
                    if let Some(h) = self.health_states.get_mut(&server_id) {
                        h.mark_dead();
                    }
                }
            }
        }
    }

    /// Handle a single command
    ///
    /// Returns `false` if the actor should shutdown.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
            McpCommand::AddTransientServer { config, respond_to } => {
                let result = self.add_transient_server(config).await;
                let _ = respond_to.send(result);
            }
            McpCommand::RemoveTransientServer {
                server_id,
                respond_to,
            } => {
                let result = self.remove_transient_server(&server_id).await;
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
            McpCommand::ListServerConfigs { respond_to } => {
                let configs = self.config.servers.values().cloned().collect::<Vec<_>>();
                let _ = respond_to.send(configs);
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
            McpCommand::AggregateInstructions { respond_to } => {
                let instructions = self.aggregate_instructions().await;
                let _ = respond_to.send(instructions);
            }
            McpCommand::Shutdown { .. } => {
                // Handled in the run() loop directly to ensure ACK is sent
                // after shutdown_all() completes
                unreachable!("Shutdown is handled in run() loop");
            }
            McpCommand::SetSamplingCallback {
                callback,
                respond_to,
            } => {
                // Set callback on all existing clients
                for client in self.clients.values() {
                    let cb = Arc::clone(&callback);
                    client
                        .set_sampling_callback(move |req| {
                            let cb = Arc::clone(&cb);
                            async move { cb(req).await }
                        })
                        .await;
                }
                // Store for new servers
                self.sampling_callback = Some(callback);
                let _ = respond_to.send(());
            }
            McpCommand::ServerListChanged { server_id, kind } => {
                self.handle_list_changed(&server_id, kind).await;
            }
        }
        true
    }

    /// Refresh caches for a server that announced a list change, then
    /// re-broadcast a typed capability event for the tool bridge.
    ///
    /// The cache is refreshed *before* the event is emitted so that the
    /// bridge's `sync_server` reads the server's current tool list.
    async fn handle_list_changed(&self, server_id: &str, kind: ListChangeKind) {
        let Some(client) = self.clients.get(server_id).cloned() else {
            tracing::debug!(
                server_id = %server_id,
                "list-changed for unknown server; ignoring"
            );
            return;
        };
        client.refresh_caches().await;
        let event = match kind {
            ListChangeKind::Tools => McpManagerEvent::ToolsChanged {
                server_id: server_id.to_string(),
                tool_count: client.list_tools().await.len(),
            },
            ListChangeKind::Resources => McpManagerEvent::ResourcesChanged {
                server_id: server_id.to_string(),
                resource_count: client.list_resources().await.len(),
            },
            ListChangeKind::Prompts => McpManagerEvent::PromptsChanged {
                server_id: server_id.to_string(),
                prompt_count: client.list_prompts().await.len(),
            },
        };
        tracing::info!(
            server_id = %server_id,
            ?kind,
            "MCP server announced a list change; re-broadcasting"
        );
        let _ = self.event_tx.send(event);
    }

    // ===== Lifecycle Methods =====

    /// Add a server configuration
    ///
    /// Upserts the config, saves to disk, and optionally starts if `auto_start` is true.
    async fn add_server(&mut self, config: McpManagerConfig) -> Result<(), String> {
        // rust-doctor-disable-next-line excessive-clone
        let server_id = config.id.clone();
        // rust-doctor-disable-next-line excessive-clone
        let server_name = config.name.clone();
        let auto_start = config.auto_start;

        // Upsert config
        // rust-doctor-disable-next-line excessive-clone
        self.config.upsert_server(config.clone());

        // Save to disk
        self.config
            .save(&self.config_path)
            .await
            .map_err(|e| format!("Failed to save config: {e}"))?;

        // Start if auto_start
        if auto_start {
            self.start_server_internal(&config).await?;
        }

        // Broadcast event after start succeeded (subscribers see consistent state)
        let _ = self.event_tx.send(McpManagerEvent::ServerAdded {
            // rust-doctor-disable-next-line excessive-clone
            server_id: server_id.clone(),
            server_name,
        });

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
            // rust-doctor-disable-next-line excessive-clone
            .map_or_else(|| server_id.to_string(), |c| c.name.clone());

        // Stop if running
        self.stop_server_internal(server_id).await;

        // Remove from config
        self.config.remove_server(server_id);

        // Save to disk
        self.config
            .save(&self.config_path)
            .await
            .map_err(|e| format!("Failed to save config: {e}"))?;

        // Broadcast event
        let _ = self.event_tx.send(McpManagerEvent::ServerRemoved {
            server_id: server_id.to_string(),
            server_name,
        });

        tracing::info!(server_id = %server_id, "Server removed");
        Ok(())
    }

    /// Add a transient (runtime-only) server.
    ///
    /// Starts the server connection without upserting or persisting the config
    /// to disk — the opposite of [`Self::add_server`]. Plugin-owned MCP servers
    /// flow through here so they never pollute the user's MCP config file.
    /// Idempotent: returns `Ok` immediately if a client with the same ID is
    /// already running, so re-syncing the plugin set never double-spawns.
    async fn add_transient_server(&mut self, config: McpManagerConfig) -> Result<(), String> {
        if self.clients.contains_key(&config.id) {
            // Already running (e.g. a previous sync). Nothing to do.
            return Ok(());
        }

        // `start_server_internal` inserts into `clients`/`health_states` and
        // emits `ServerStarted`, which the tool bridge turns into tool
        // registrations — exactly the path persisted servers use, minus the
        // `self.config` upsert + disk save.
        self.start_server_internal(&config).await?;

        tracing::info!(server_id = %config.id, "Transient server added");
        Ok(())
    }

    /// Remove a transient server without touching the persisted config.
    ///
    /// Stops the running client (if any), drops its health state, and emits
    /// `ServerRemoved` so the tool bridge unregisters the server's tools. A
    /// no-op for an unknown ID. Never reads or writes the on-disk config.
    async fn remove_transient_server(&mut self, server_id: &str) -> Result<(), String> {
        let was_running = self.clients.contains_key(server_id);
        self.stop_server_internal(server_id).await;
        // Transient servers have no config-backed health entry to retain.
        self.health_states.remove(server_id);

        if was_running {
            let _ = self.event_tx.send(McpManagerEvent::ServerRemoved {
                server_id: server_id.to_string(),
                server_name: server_id.to_string(),
            });
            tracing::info!(server_id = %server_id, "Transient server removed");
        }
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
            .ok_or_else(|| format!("Server not found: {server_id}"))?;

        // rust-doctor-disable-next-line excessive-clone
        let server_name = config.name.clone();

        // Update health state (insert if not yet tracked)
        self.health_states
            .entry(server_id.to_string())
            .or_default()
            .mark_restarting();

        // Broadcast restarting event
        let attempt = self
            .health_states
            .get(server_id)
            .map_or(1, |h| h.restart_count);
        let _ = self.event_tx.send(McpManagerEvent::ServerRestarting {
            server_id: server_id.to_string(),
            server_name,
            attempt,
        });

        // Stop the server
        self.stop_server_internal(server_id).await;

        // Start the server immediately — a restart delay would have to sleep,
        // but `restart_server` runs inside the actor's `tokio::select!` loop
        // (via `health_check_pass`), so blocking here would stall command
        // processing. The restart-window cap (`max_restarts`) bounds churn
        // instead.
        self.start_server_internal(&config).await?;

        tracing::info!(server_id = %server_id, "Server restarted");
        Ok(())
    }

    /// Start a stopped server
    async fn start_server(&mut self, server_id: &str) -> Result<(), String> {
        // Check if already running
        if self.clients.contains_key(server_id) {
            return Err(format!("Server already running: {server_id}"));
        }

        let config = self
            .config
            .get_server(server_id)
            .cloned()
            .ok_or_else(|| format!("Server not found: {server_id}"))?;

        self.start_server_internal(&config).await
    }

    /// Stop a running server
    async fn stop_server(&mut self, server_id: &str) -> Result<(), String> {
        if !self.clients.contains_key(server_id) {
            return Err(format!("Server not running: {server_id}"));
        }

        let server_name = self
            .config
            .get_server(server_id)
            // rust-doctor-disable-next-line excessive-clone
            .map_or_else(|| server_id.to_string(), |c| c.name.clone());

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
    /// Creates an `McpClient` and connects using the appropriate transport.
    async fn start_server_internal(&mut self, config: &McpManagerConfig) -> Result<(), String> {
        // Install the per-server tool filter before the client is shared, so
        // `list_tools` (and thus registration, aggregation, and counts) only
        // ever surface the tools this server is allowed to expose.
        let mut client = McpClient::new();
        // rust-doctor-disable-next-line excessive-clone
        client.set_tool_filter(config.tool_filter.clone());
        let client = Arc::new(client);

        // Start based on transport type
        match config.transport {
            McpTransportType::Stdio => {
                let command = config.command.as_ref().ok_or_else(|| {
                    format!("No command specified for stdio server: {}", config.id)
                })?;

                // Resolve `{{secret:NAME}}` env references into this child's
                // env only — never the daemon's own process env.
                let resolved_env = super::secret_resolver::resolve_secret_map(
                    &config.env,
                    self.secret_resolver.as_deref(),
                )
                .await;

                let external_config = ExternalServerConfig {
                    // rust-doctor-disable-next-line excessive-clone
                    name: config.id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    command: command.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    args: config.args.clone(),
                    env: resolved_env,
                    cwd: None,
                    // rust-doctor-disable-next-line excessive-clone
                    requires_runtime: config.requires_runtime.clone(),
                    timeout_seconds: config.timeout_seconds,
                };

                client
                    .start_external_server(external_config)
                    .await
                    .map_err(|e| format!("Failed to start stdio server: {e}"))?;
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

                // Resolve `{{secret:NAME}}` header references (Authorization,
                // API keys) the same way stdio env is resolved — the plaintext
                // only ever exists in this request's header map.
                let resolved_headers = super::secret_resolver::resolve_secret_map(
                    &config.headers,
                    self.secret_resolver.as_deref(),
                )
                .await;

                let mut remote_config =
                    McpRemoteServerConfig::new(&config.id, url).with_transport(transport);
                remote_config.headers = resolved_headers;

                let remote_config = if let Some(timeout) = config.timeout_seconds {
                    remote_config.with_timeout(timeout)
                } else {
                    remote_config
                };

                client
                    .start_remote_server(remote_config)
                    .await
                    .map_err(|e| format!("Failed to start remote server: {e}"))?;
            }
        }

        // Route this server's `*/list_changed` notifications back into the
        // actor so the tool registry and capability state stay in sync.
        // rust-doctor-disable-next-line excessive-clone
        let cmd_tx = self.cmd_tx.clone();
        // rust-doctor-disable-next-line excessive-clone
        let notify_server_id = config.id.clone();
        let notification_handler: crate::mcp::transport::NotificationCallback =
            Box::new(move |notification| {
                if let Some(kind) = classify_list_change(&notification.method) {
                    // Fire-and-forget: a dropped signal self-heals on the next
                    // notification, a restart, or the periodic health probe.
                    let _ = cmd_tx.try_send(McpCommand::ServerListChanged {
                        // rust-doctor-disable-next-line excessive-clone
                        server_id: notify_server_id.clone(),
                        kind,
                    });
                }
            });
        client
            .set_notification_handler(&config.id, notification_handler)
            .await;

        // Set sampling callback if one is registered
        if let Some(ref callback) = self.sampling_callback {
            let cb = Arc::clone(callback);
            client
                .set_sampling_callback(move |req| {
                    let cb = Arc::clone(&cb);
                    async move { cb(req).await }
                })
                .await;
        }

        // Get tool count for event
        let tool_count = client.list_tools().await.len();

        // Store client
        // rust-doctor-disable-next-line excessive-clone
        let server_id = config.id.clone();
        // rust-doctor-disable-next-line excessive-clone
        self.clients.insert(server_id.clone(), client);
        // Mark healthy while preserving the restart-window bookkeeping
        // (restart_count / restart_window_start). Re-inserting a fresh
        // ServerHealth here would zero the counter on every successful spawn,
        // letting a server that starts fine but dies between probes evade the
        // max_restarts cap and restart-loop forever.
        self.health_states
            // rust-doctor-disable-next-line excessive-clone
            .entry(server_id.clone())
            .or_default()
            .record_success();

        // Broadcast started event
        let _ = self.event_tx.send(McpManagerEvent::ServerStarted {
            server_id,
            // rust-doctor-disable-next-line excessive-clone
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
    /// Calls `client.stop_all()` and removes from tracking maps.
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
                .map_or(HealthStatus::Stopped, |h| h.status);

            // Get tool/resource/template/prompt counts from active clients
            let (tool_count, resource_count, resource_template_count, prompt_count) =
                if let Some(client) = self.clients.get(id) {
                    let tools = client.list_tools().await.len();
                    let resources = client.list_resources().await.len();
                    let templates = client.list_resource_templates().await.len();
                    let prompts = client.list_prompts().await.len();
                    (tools, resources, templates, prompts)
                } else {
                    (0, 0, 0, 0)
                };

            servers.push(McpServerInfo {
                // rust-doctor-disable-next-line excessive-clone
                id: id.clone(),
                // rust-doctor-disable-next-line excessive-clone
                name: config.name.clone(),
                transport: config.transport,
                tool_count,
                resource_count,
                resource_template_count,
                prompt_count,
                health,
            });
        }

        // Transient servers (plugin-owned, runtime-only) live in `clients` but
        // not in `self.config`. Surface them too so `mcp.list` reflects reality
        // and the tool bridge's lag-recovery `resync_all` can re-fetch their
        // tools after a dropped event.
        for (id, client) in &self.clients {
            if self.config.servers.contains_key(id) {
                continue;
            }
            let health = self
                .health_states
                .get(id)
                .map_or(HealthStatus::Healthy, |h| h.status);
            // rust-doctor-disable-next-line excessive-clone
            let id = id.clone();
            servers.push(McpServerInfo {
                // rust-doctor-disable-next-line excessive-clone
                id: id.clone(),
                name: id,
                transport: McpTransportType::Stdio,
                tool_count: client.list_tools().await.len(),
                resource_count: client.list_resources().await.len(),
                resource_template_count: client.list_resource_templates().await.len(),
                prompt_count: client.list_prompts().await.len(),
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

        // rust-doctor-disable-next-line excessive-clone
        let config = config.clone();
        Some(McpServerStatusDetail {
            id: server_id.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            name: config.name.clone(),
            transport: config.transport,
            health,
            tools,
            resources,
            prompts,
            config,
        })
    }

    // ===== Aggregation Methods =====

    /// Aggregate items from all healthy servers using the provided accessor.
    async fn aggregate_from_healthy<T, F, Fut>(&self, accessor: F) -> Vec<T>
    where
        F: Fn(Arc<McpClient>) -> Fut,
        Fut: std::future::Future<Output = Vec<T>>,
    {
        let mut result = Vec::new();

        for (server_id, client) in &self.clients {
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(
                    health.status,
                    HealthStatus::Healthy | HealthStatus::Degraded { .. }
                ) {
                    continue;
                }
            }

            result.extend(accessor(Arc::clone(client)).await);
        }

        result
    }

    /// Aggregate tools from all healthy servers
    async fn aggregate_tools(&self) -> Vec<McpTool> {
        self.aggregate_from_healthy(|c| async move { c.list_tools().await })
            .await
    }

    /// Aggregate resources from all healthy servers
    async fn aggregate_resources(&self) -> Vec<McpResource> {
        self.aggregate_from_healthy(|c| async move { c.list_resources().await })
            .await
    }

    /// Aggregate prompts from all healthy servers
    async fn aggregate_prompts(&self) -> Vec<McpPrompt> {
        self.aggregate_from_healthy(|c| async move { c.list_prompts().await })
            .await
    }

    /// Aggregate server-provided `instructions` from all healthy servers.
    /// Each per-server `McpClient` owns one connection, so collecting across
    /// every healthy client yields the full set of connected-server guidance
    /// that `McpInstructionsLayer` renders into the system prompt.
    async fn aggregate_instructions(
        &self,
    ) -> Vec<crate::thinker::prompt_layer::McpServerInstruction> {
        self.aggregate_from_healthy(|c| async move { c.collect_instructions().await })
            .await
    }
}

/// Map a TTL-driven refresh report onto the list-changed kinds it implies.
///
/// Resource templates share the resources signal, which is why
/// [`ChangedLists`] carries no separate flag for them.
fn changed_list_kinds(changed: crate::mcp::external::ChangedLists) -> Vec<ListChangeKind> {
    let mut kinds = Vec::new();
    if changed.tools {
        kinds.push(ListChangeKind::Tools);
    }
    if changed.resources {
        kinds.push(ListChangeKind::Resources);
    }
    if changed.prompts {
        kinds.push(ListChangeKind::Prompts);
    }
    kinds
}

/// Map an MCP notification method to the capability list it changed, if any.
///
/// Both the spec's `snake_case` form (`list_changed`) and the camelCase form
/// some servers emit (`listChanged`) are accepted.
fn classify_list_change(method: &str) -> Option<ListChangeKind> {
    match method {
        "notifications/tools/list_changed" | "notifications/tools/listChanged" => {
            Some(ListChangeKind::Tools)
        }
        "notifications/resources/list_changed" | "notifications/resources/listChanged" => {
            Some(ListChangeKind::Resources)
        }
        "notifications/prompts/list_changed" | "notifications/prompts/listChanged" => {
            Some(ListChangeKind::Prompts)
        }
        _ => None,
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
        assert_eq!(config.max_failures, 3);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.restart_window, Duration::from_secs(300));
    }

    #[test]
    fn classify_list_change_recognizes_both_casings() {
        assert_eq!(
            classify_list_change("notifications/tools/list_changed"),
            Some(ListChangeKind::Tools)
        );
        assert_eq!(
            classify_list_change("notifications/tools/listChanged"),
            Some(ListChangeKind::Tools)
        );
        assert_eq!(
            classify_list_change("notifications/resources/list_changed"),
            Some(ListChangeKind::Resources)
        );
        assert_eq!(
            classify_list_change("notifications/prompts/listChanged"),
            Some(ListChangeKind::Prompts)
        );
        assert_eq!(classify_list_change("notifications/progress"), None);
        assert_eq!(classify_list_change("notifications/initialized"), None);
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
    async fn add_transient_server_is_idempotent_and_never_persists() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");
        let (mut actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();

        // Simulate an already-running transient server (plugin-owned id).
        let id = "plugin:demo/srv";
        actor
            .clients
            .insert(id.to_string(), Arc::new(McpClient::new()));

        // Same id already running → Ok without re-spawning, and crucially the
        // persisted config is never written for a transient server.
        let cfg = McpManagerConfig::stdio(id, "demo", "echo");
        assert!(actor.add_transient_server(cfg).await.is_ok());
        assert!(
            actor.config.servers.is_empty(),
            "transient server must never be persisted to config"
        );
    }

    #[tokio::test]
    async fn remove_transient_server_drops_client_without_touching_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");
        let (mut actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();

        let id = "plugin:demo/srv";
        actor
            .clients
            .insert(id.to_string(), Arc::new(McpClient::new()));
        actor
            .health_states
            .insert(id.to_string(), ServerHealth::default());

        assert!(actor.remove_transient_server(id).await.is_ok());
        assert!(!actor.clients.contains_key(id), "client should be removed");
        assert!(
            !actor.health_states.contains_key(id),
            "health state should be dropped"
        );
        assert!(actor.config.servers.is_empty());

        // Removing an unknown id is a harmless no-op.
        assert!(actor.remove_transient_server("plugin:nope/x").await.is_ok());
    }

    #[tokio::test]
    async fn list_servers_includes_transient_clients() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("mcp_config.json");
        let (mut actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();

        // A transient server lives in `clients` but not in `config`.
        let id = "plugin:demo/srv";
        actor
            .clients
            .insert(id.to_string(), Arc::new(McpClient::new()));

        let servers = actor.list_servers().await;
        assert_eq!(servers.len(), 1, "transient client should be listed");
        assert_eq!(servers[0].id, id);
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

    #[tokio::test]
    async fn list_server_configs_returns_persisted_configs() {
        let path = std::env::temp_dir().join(format!("aleph_mcp_cfgs_{}.json", std::process::id()));
        let _ = tokio::fs::remove_file(&path).await;
        let (actor, handle) = McpManagerActor::new(Some(path.clone()))
            .await
            .expect("actor builds");
        tokio::spawn(actor.run());

        handle
            .add_server(
                McpManagerConfig::stdio("srv-a", "Server A", "/bin/true").with_auto_start(false),
            )
            .await
            .expect("add_server");

        let configs = handle.list_server_configs().await.expect("list configs");
        assert!(configs
            .iter()
            .any(|c| c.id == "srv-a" && c.name == "Server A"));

        let _ = tokio::fs::remove_file(&path).await;
    }
}

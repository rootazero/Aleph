//! MCP Manager Types
//!
//! Types for the `McpManager` actor, including commands, events, and server configuration.
//!
//! This module provides the foundational types for MCP orchestration:
//! - `McpManagerConfig` - Persistence-friendly server configuration
//! - `McpCommand` - Actor command enum for communication
//! - `McpManagerEvent` - Extended events for manager lifecycle
//! - Health tracking types for circuit breaker pattern

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::mcp::{McpClient, McpPrompt, McpResource, McpTool, McpToolFilter};

/// Transport type for MCP servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    /// Standard I/O transport (subprocess)
    #[default]
    Stdio,
    /// HTTP transport
    Http,
    /// Server-Sent Events transport
    Sse,
}

impl std::fmt::Display for McpTransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio => write!(f, "stdio"),
            Self::Http => write!(f, "http"),
            Self::Sse => write!(f, "sse"),
        }
    }
}

/// MCP server configuration for persistence and actor management
///
/// This configuration is designed to be serializable for storage in config files
/// and provides all necessary information to start and manage an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpManagerConfig {
    /// Unique server identifier
    pub id: String,
    /// Human-readable server name
    pub name: String,
    /// Transport type (stdio, http, sse)
    #[serde(default)]
    pub transport: McpTransportType,
    /// Command to execute (for stdio transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// URL for remote servers (http/sse transport)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Custom HTTP headers for remote (http/sse) transports — auth tokens, API
    /// keys. Values may carry `{{secret:NAME}}` references, resolved per-spawn
    /// exactly like [`Self::env`]; plaintext secrets are never persisted here.
    /// Ignored for stdio.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    /// Required runtime (e.g., "node", "python", "bun")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_runtime: Option<String>,
    /// Whether to auto-start this server
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// Request timeout in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Optional allow/deny filter over the tools this server exposes.
    /// Absent = expose every advertised tool (backward-compatible default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_filter: Option<McpToolFilter>,
}

const fn default_true() -> bool {
    true
}

impl McpManagerConfig {
    /// Create a new stdio server configuration
    pub fn stdio(
        id: impl Into<String>,
        name: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransportType::Stdio,
            command: Some(command.into()),
            args: Vec::new(),
            url: None,
            env: HashMap::new(),
            headers: HashMap::new(),
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
            tool_filter: None,
        }
    }

    /// Create a new HTTP server configuration
    pub fn http(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransportType::Http,
            command: None,
            args: Vec::new(),
            url: Some(url.into()),
            env: HashMap::new(),
            headers: HashMap::new(),
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
            tool_filter: None,
        }
    }

    /// Create a new SSE server configuration
    pub fn sse(id: impl Into<String>, name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransportType::Sse,
            command: None,
            args: Vec::new(),
            url: Some(url.into()),
            env: HashMap::new(),
            headers: HashMap::new(),
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
            tool_filter: None,
        }
    }

    /// Set command arguments
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Set environment variables
    #[must_use]
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set custom HTTP headers (remote transports only). Values may be
    /// `{{secret:NAME}}` references — they resolve at spawn, not here.
    #[must_use]
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// Set required runtime
    pub fn with_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.requires_runtime = Some(runtime.into());
        self
    }

    /// Set auto-start flag
    #[must_use]
    pub const fn with_auto_start(mut self, auto_start: bool) -> Self {
        self.auto_start = auto_start;
        self
    }

    /// Set timeout in seconds
    #[must_use]
    pub const fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Set the per-server tool allow/deny filter.
    #[must_use]
    pub fn with_tool_filter(mut self, filter: McpToolFilter) -> Self {
        self.tool_filter = Some(filter);
        self
    }
}

/// Server information for listing (lightweight, serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    /// Server identifier
    pub id: String,
    /// Server name
    pub name: String,
    /// Transport type
    pub transport: McpTransportType,
    /// Number of tools provided
    pub tool_count: usize,
    /// Number of resources provided
    pub resource_count: usize,
    /// Number of resource templates provided (parameterized URIs). `#[serde(default)]`
    /// keeps older serialized `McpServerInfo` (pre-templates) deserializable.
    #[serde(default)]
    pub resource_template_count: usize,
    /// Number of prompts provided
    pub prompt_count: usize,
    /// Current health status
    pub health: HealthStatus,
}

/// Health status for circuit breaker pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
#[derive(Default)]
pub enum HealthStatus {
    /// Server is healthy
    Healthy,
    /// Server is degraded (some failures)
    Degraded {
        /// Number of consecutive failures
        failures: u32,
    },
    /// Server is unhealthy (circuit open)
    Unhealthy,
    /// Server is restarting
    Restarting {
        /// Current restart attempt number
        attempt: u32,
    },
    /// Server is dead (max restarts exceeded)
    Dead,
    /// Server is stopped (intentionally)
    #[default]
    Stopped,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded { failures } => write!(f, "degraded ({failures} failures)"),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Restarting { attempt } => write!(f, "restarting (attempt {attempt})"),
            Self::Dead => write!(f, "dead"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Server health tracking for circuit breaker
#[derive(Debug, Clone)]
pub struct ServerHealth {
    /// Number of consecutive failures
    pub consecutive_failures: u32,
    /// Last health check time
    pub last_check: Option<Instant>,
    /// Number of restarts in current window
    pub restart_count: u32,
    /// Start of restart window
    pub restart_window_start: Option<Instant>,
    /// Current health status
    pub status: HealthStatus,
    /// Last error message
    pub last_error: Option<String>,
}

impl Default for ServerHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_check: None,
            restart_count: 0,
            restart_window_start: None,
            status: HealthStatus::Stopped,
            last_error: None,
        }
    }
}

impl ServerHealth {
    /// Create a new healthy server health
    #[must_use]
    pub fn healthy() -> Self {
        Self {
            status: HealthStatus::Healthy,
            ..Default::default()
        }
    }

    /// Record a successful operation
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_check = Some(Instant::now());
        self.status = HealthStatus::Healthy;
        self.last_error = None;
    }

    /// Record a failure.
    ///
    /// `max_failures` is the consecutive-failure count at which the server is
    /// marked [`HealthStatus::Unhealthy`]; below it (but past the first) the
    /// server is [`HealthStatus::Degraded`]. Both states are restart-eligible
    /// (see [`Self::should_restart`]), so the threshold governs the label and
    /// the "fully unhealthy" point rather than the restart cadence.
    pub fn record_failure(&mut self, error: impl Into<String>, max_failures: u32) {
        self.consecutive_failures += 1;
        self.last_check = Some(Instant::now());
        self.last_error = Some(error.into());

        // Keep a Degraded band below the unhealthy threshold so a single blip
        // does not immediately read as fully unhealthy.
        let unhealthy_at = max_failures.max(2);
        self.status = if self.consecutive_failures >= unhealthy_at {
            HealthStatus::Unhealthy
        } else if self.consecutive_failures >= 2 {
            HealthStatus::Degraded {
                failures: self.consecutive_failures,
            }
        } else {
            HealthStatus::Healthy
        };
    }

    /// Mark as restarting
    pub fn mark_restarting(&mut self) {
        self.restart_count += 1;
        self.status = HealthStatus::Restarting {
            attempt: self.restart_count,
        };

        // Initialize restart window if not set
        if self.restart_window_start.is_none() {
            self.restart_window_start = Some(Instant::now());
        }
    }

    /// Mark as dead (max restarts exceeded)
    pub const fn mark_dead(&mut self) {
        self.status = HealthStatus::Dead;
    }

    /// Mark as stopped
    pub const fn mark_stopped(&mut self) {
        self.status = HealthStatus::Stopped;
        self.consecutive_failures = 0;
    }

    /// Check if server should be restarted
    ///
    /// Also resets the restart window if it has expired, allowing the server
    /// a fresh set of restart attempts after a quiet period.
    pub fn should_restart(&mut self, max_restarts: u32, window_seconds: u64) -> bool {
        // Reset expired window first so the counter starts fresh
        self.maybe_reset_window(window_seconds);

        match self.status {
            HealthStatus::Unhealthy | HealthStatus::Degraded { .. } => {
                if self.restart_window_start.is_some() {
                    // Within window, check count
                    self.restart_count < max_restarts
                } else {
                    // No window yet, allow restart
                    true
                }
            }
            HealthStatus::Dead | HealthStatus::Stopped | HealthStatus::Restarting { .. } => false,
            HealthStatus::Healthy => false,
        }
    }

    /// Reset restart window if expired
    fn maybe_reset_window(&mut self, window_seconds: u64) {
        if let Some(start) = self.restart_window_start {
            if start.elapsed().as_secs() > window_seconds {
                self.restart_count = 0;
                self.restart_window_start = None;
            }
        }
    }
}

/// Detailed server status information
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatusDetail {
    /// Server identifier
    pub id: String,
    /// Server name
    pub name: String,
    /// Transport type
    pub transport: McpTransportType,
    /// Current health
    pub health: ServerHealth,
    /// Available tools
    pub tools: Vec<McpTool>,
    /// Available resources
    pub resources: Vec<McpResource>,
    /// Available prompts
    pub prompts: Vec<McpPrompt>,
    /// Server configuration
    pub config: McpManagerConfig,
}

// Custom serialization for ServerHealth since Instant isn't serializable
impl Serialize for ServerHealth {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ServerHealth", 5)?;
        state.serialize_field("consecutive_failures", &self.consecutive_failures)?;
        state.serialize_field("restart_count", &self.restart_count)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("last_error", &self.last_error)?;
        // Convert Instant to elapsed seconds for serialization
        state.serialize_field(
            "seconds_since_check",
            &self.last_check.map(|i| i.elapsed().as_secs()),
        )?;
        state.end()
    }
}

/// Which capability list a server announced as changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChangeKind {
    /// The server's `tools/list` changed.
    Tools,
    /// The server's `resources/list` changed.
    Resources,
    /// The server's `prompts/list` changed.
    Prompts,
}

/// Commands for the MCP Manager Actor
///
/// These commands are sent via channels to control the manager's behavior.
/// Each command that expects a response includes a oneshot channel for the reply.
pub enum McpCommand {
    /// Add a new server configuration
    AddServer {
        /// Server configuration
        config: McpManagerConfig,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Remove a server by ID
    RemoveServer {
        /// Server ID to remove
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Add a **transient** (runtime-only) server.
    ///
    /// Unlike [`McpCommand::AddServer`], the config is NOT persisted to the
    /// on-disk MCP config and survives only for the life of the process. Used
    /// for plugin-owned MCP servers whose lifecycle is governed by the plugin
    /// system, not the user's MCP config file. Idempotent: a no-op if a server
    /// with the same ID is already running.
    AddTransientServer {
        /// Server configuration (not persisted)
        config: McpManagerConfig,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Remove a transient server by ID without touching the persisted config.
    ///
    /// Stops the running client (if any) and drops its health state. A no-op
    /// if the server is not running. The persisted MCP config is never read or
    /// written, so this is safe to call for plugin-owned servers.
    RemoveTransientServer {
        /// Transient server ID to remove
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Restart a specific server
    RestartServer {
        /// Server ID to restart
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Start a stopped server
    StartServer {
        /// Server ID to start
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Stop a running server
    StopServer {
        /// Server ID to stop
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Get the `McpClient` for a specific server
    GetClient {
        /// Server ID
        server_id: String,
        /// Response channel (returns Arc<McpClient> if available)
        respond_to: oneshot::Sender<Option<Arc<McpClient>>>,
    },

    /// List all servers
    ListServers {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpServerInfo>>,
    },

    /// List all **persisted** server configurations (full config, not the
    /// lightweight `McpServerInfo`). Excludes transient plugin-owned servers,
    /// which live only in `clients`. Used by the Settings MCP page to render
    /// editable command/args/env without per-server status round-trips.
    ListServerConfigs {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpManagerConfig>>,
    },

    /// Get detailed status for a server
    GetStatus {
        /// Server ID
        server_id: String,
        /// Response channel
        respond_to: oneshot::Sender<Option<McpServerStatusDetail>>,
    },

    /// Get aggregated tools from all servers
    AggregateTools {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpTool>>,
    },

    /// Get aggregated resources from all servers
    AggregateResources {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpResource>>,
    },

    /// Get aggregated prompts from all servers
    AggregatePrompts {
        /// Response channel
        respond_to: oneshot::Sender<Vec<McpPrompt>>,
    },

    /// Get aggregated server-provided `instructions` from all servers.
    /// Feeds `McpInstructionsLayer` so each connected server's usage guidance
    /// reaches the system prompt.
    AggregateInstructions {
        /// Response channel
        respond_to: oneshot::Sender<Vec<crate::thinker::prompt_layer::McpServerInstruction>>,
    },

    /// Reload configuration from disk
    ReloadConfig {
        /// Response channel
        respond_to: oneshot::Sender<Result<(), String>>,
    },

    /// Graceful shutdown
    Shutdown {
        /// Response channel (sent when shutdown complete)
        respond_to: oneshot::Sender<()>,
    },

    /// Set sampling callback for all servers
    SetSamplingCallback {
        callback: Arc<crate::mcp::sampling::SamplingCallback>,
        respond_to: oneshot::Sender<()>,
    },

    /// A server announced that one of its capability lists changed.
    ///
    /// Sent fire-and-forget from a transport notification handler; the actor
    /// re-fetches the affected caches and re-broadcasts a typed event.
    ServerListChanged {
        /// Server ID that emitted the notification
        server_id: String,
        /// Which list changed
        kind: ListChangeKind,
    },
}

impl std::fmt::Debug for McpCommand {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddServer { config, .. } => {
                f.debug_struct("AddServer").field("config", config).finish()
            }
            Self::RemoveServer { server_id, .. } => f
                .debug_struct("RemoveServer")
                .field("server_id", server_id)
                .finish(),
            Self::AddTransientServer { config, .. } => f
                .debug_struct("AddTransientServer")
                .field("config", config)
                .finish(),
            Self::RemoveTransientServer { server_id, .. } => f
                .debug_struct("RemoveTransientServer")
                .field("server_id", server_id)
                .finish(),
            Self::RestartServer { server_id, .. } => f
                .debug_struct("RestartServer")
                .field("server_id", server_id)
                .finish(),
            Self::StartServer { server_id, .. } => f
                .debug_struct("StartServer")
                .field("server_id", server_id)
                .finish(),
            Self::StopServer { server_id, .. } => f
                .debug_struct("StopServer")
                .field("server_id", server_id)
                .finish(),
            Self::GetClient { server_id, .. } => f
                .debug_struct("GetClient")
                .field("server_id", server_id)
                .finish(),
            Self::ListServers { .. } => f.debug_struct("ListServers").finish(),
            Self::ListServerConfigs { .. } => f.debug_struct("ListServerConfigs").finish(),
            Self::GetStatus { server_id, .. } => f
                .debug_struct("GetStatus")
                .field("server_id", server_id)
                .finish(),
            Self::AggregateTools { .. } => f.debug_struct("AggregateTools").finish(),
            Self::AggregateResources { .. } => f.debug_struct("AggregateResources").finish(),
            Self::AggregatePrompts { .. } => f.debug_struct("AggregatePrompts").finish(),
            Self::AggregateInstructions { .. } => f.debug_struct("AggregateInstructions").finish(),
            Self::ReloadConfig { .. } => f.debug_struct("ReloadConfig").finish(),
            Self::Shutdown { .. } => f.debug_struct("Shutdown").finish(),
            Self::SetSamplingCallback { .. } => f.debug_struct("SetSamplingCallback").finish(),
            Self::ServerListChanged { server_id, kind } => f
                .debug_struct("ServerListChanged")
                .field("server_id", server_id)
                .field("kind", kind)
                .finish(),
        }
    }
}

/// Events emitted by the MCP Manager
///
/// These events are broadcast to interested subscribers (e.g., Gateway, UI)
/// to notify them of changes in the manager's state.
#[derive(Debug, Clone)]
pub enum McpManagerEvent {
    /// Manager has finished initialization
    ManagerReady,

    /// Manager is shutting down
    ManagerShutdown,

    /// A server was added
    ServerAdded {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
    },

    /// A server was removed
    ServerRemoved {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
    },

    /// A server started successfully
    ServerStarted {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
        /// Number of tools available
        tool_count: usize,
    },

    /// A server stopped
    ServerStopped {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
    },

    /// A server crashed
    ServerCrashed {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
        /// Error message
        error: String,
    },

    /// A server is being restarted
    ServerRestarting {
        /// Server ID
        server_id: String,
        /// Server name
        server_name: String,
        /// Restart attempt number
        attempt: u32,
    },

    /// Tools changed on a server
    ToolsChanged {
        /// Server ID
        server_id: String,
        /// New tool count
        tool_count: usize,
    },

    /// Resources changed on a server
    ResourcesChanged {
        /// Server ID
        server_id: String,
        /// New resource count
        resource_count: usize,
    },

    /// Prompts changed on a server
    PromptsChanged {
        /// Server ID
        server_id: String,
        /// New prompt count
        prompt_count: usize,
    },

    /// Configuration was reloaded
    ConfigReloaded {
        /// Number of servers after reload
        server_count: usize,
    },
}

impl McpManagerEvent {
    /// Get the server ID if this event is server-specific
    #[must_use]
    pub fn server_id(&self) -> Option<&str> {
        match self {
            Self::ManagerReady | Self::ManagerShutdown | Self::ConfigReloaded { .. } => None,
            Self::ServerAdded { server_id, .. }
            | Self::ServerRemoved { server_id, .. }
            | Self::ServerStarted { server_id, .. }
            | Self::ServerStopped { server_id, .. }
            | Self::ServerCrashed { server_id, .. }
            | Self::ServerRestarting { server_id, .. }
            | Self::ToolsChanged { server_id, .. }
            | Self::ResourcesChanged { server_id, .. }
            | Self::PromptsChanged { server_id, .. } => Some(server_id),
        }
    }

    /// Check if this is a lifecycle event (start/stop/crash)
    #[must_use]
    pub const fn is_lifecycle_event(&self) -> bool {
        matches!(
            self,
            Self::ServerStarted { .. }
                | Self::ServerStopped { .. }
                | Self::ServerCrashed { .. }
                | Self::ServerRestarting { .. }
        )
    }

    /// Check if this is a capability change event
    #[must_use]
    pub const fn is_capability_event(&self) -> bool {
        matches!(
            self,
            Self::ToolsChanged { .. } | Self::ResourcesChanged { .. } | Self::PromptsChanged { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_transport_type_display() {
        assert_eq!(format!("{}", McpTransportType::Stdio), "stdio");
        assert_eq!(format!("{}", McpTransportType::Http), "http");
        assert_eq!(format!("{}", McpTransportType::Sse), "sse");
    }

    #[test]
    fn test_mcp_manager_config_stdio() {
        let config = McpManagerConfig::stdio("test-id", "Test Server", "/usr/bin/test");
        assert_eq!(config.id, "test-id");
        assert_eq!(config.name, "Test Server");
        assert_eq!(config.transport, McpTransportType::Stdio);
        assert_eq!(config.command, Some("/usr/bin/test".to_string()));
        assert!(config.auto_start);
    }

    #[test]
    fn test_mcp_manager_config_http() {
        let config =
            McpManagerConfig::http("remote-id", "Remote Server", "https://api.example.com/mcp");
        assert_eq!(config.id, "remote-id");
        assert_eq!(config.transport, McpTransportType::Http);
        assert_eq!(config.url, Some("https://api.example.com/mcp".to_string()));
    }

    #[test]
    fn test_mcp_manager_config_builder() {
        let config = McpManagerConfig::stdio("id", "name", "cmd")
            .with_args(vec!["--verbose".to_string()])
            .with_runtime("node")
            .with_timeout(60)
            .with_auto_start(false);

        assert_eq!(config.args, vec!["--verbose"]);
        assert_eq!(config.requires_runtime, Some("node".to_string()));
        assert_eq!(config.timeout_seconds, Some(60));
        assert!(!config.auto_start);
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        assert_eq!(
            format!("{}", HealthStatus::Degraded { failures: 3 }),
            "degraded (3 failures)"
        );
        assert_eq!(
            format!("{}", HealthStatus::Restarting { attempt: 2 }),
            "restarting (attempt 2)"
        );
    }

    #[test]
    fn test_server_health_record_success() {
        let mut health = ServerHealth {
            consecutive_failures: 5,
            status: HealthStatus::Unhealthy,
            ..ServerHealth::default()
        };

        health.record_success();

        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.last_error.is_none());
    }

    #[test]
    fn test_server_health_record_failure() {
        let mut health = ServerHealth {
            status: HealthStatus::Healthy,
            ..ServerHealth::default()
        };

        // First failure - still healthy (threshold = 5)
        health.record_failure("error 1", 5);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.status, HealthStatus::Healthy);

        // Second failure - degraded
        health.record_failure("error 2", 5);
        assert_eq!(health.consecutive_failures, 2);
        assert!(matches!(
            health.status,
            HealthStatus::Degraded { failures: 2 }
        ));

        // Reaching the threshold - unhealthy
        health.record_failure("error 3", 5);
        health.record_failure("error 4", 5);
        health.record_failure("error 5", 5);
        assert_eq!(health.consecutive_failures, 5);
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_server_health_restarting() {
        let mut health = ServerHealth::default();

        health.mark_restarting();
        assert_eq!(health.restart_count, 1);
        assert!(matches!(
            health.status,
            HealthStatus::Restarting { attempt: 1 }
        ));

        health.mark_restarting();
        assert_eq!(health.restart_count, 2);
        assert!(matches!(
            health.status,
            HealthStatus::Restarting { attempt: 2 }
        ));
    }

    #[test]
    fn test_mcp_manager_event_server_id() {
        let event = McpManagerEvent::ServerStarted {
            server_id: "test".to_string(),
            server_name: "Test".to_string(),
            tool_count: 5,
        };
        assert_eq!(event.server_id(), Some("test"));

        let event = McpManagerEvent::ManagerReady;
        assert_eq!(event.server_id(), None);
    }

    #[test]
    fn test_mcp_manager_event_classification() {
        let lifecycle = McpManagerEvent::ServerStarted {
            server_id: "test".to_string(),
            server_name: "Test".to_string(),
            tool_count: 5,
        };
        assert!(lifecycle.is_lifecycle_event());
        assert!(!lifecycle.is_capability_event());

        let capability = McpManagerEvent::ToolsChanged {
            server_id: "test".to_string(),
            tool_count: 10,
        };
        assert!(!capability.is_lifecycle_event());
        assert!(capability.is_capability_event());
    }

    #[test]
    fn test_mcp_manager_config_serialization() {
        let config = McpManagerConfig::stdio("test", "Test", "/usr/bin/test")
            .with_args(vec!["--verbose".to_string()])
            .with_runtime("node");

        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("\"id\": \"test\""));
        assert!(json.contains("\"transport\": \"stdio\""));
        assert!(json.contains("\"requires_runtime\": \"node\""));

        let deserialized: McpManagerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, config.id);
        assert_eq!(deserialized.transport, config.transport);
    }

    #[test]
    fn tool_filter_builds_and_round_trips_serde() {
        let cfg = McpManagerConfig::stdio("id", "name", "cmd").with_tool_filter(McpToolFilter {
            allow: vec!["read_*".to_string()],
            deny: vec!["*_delete".to_string()],
        });
        let filter = cfg.tool_filter.clone().expect("filter set");
        assert!(filter.allows("read_file"));
        assert!(!filter.allows("read_delete"));

        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("tool_filter"));
        let back: McpManagerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_filter, cfg.tool_filter);
    }

    #[test]
    fn tool_filter_absent_deserializes_to_none() {
        // Legacy config JSON without the field must still load (backward compat).
        let json = r#"{"id":"x","name":"X","transport":"stdio","command":"cmd"}"#;
        let cfg: McpManagerConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.tool_filter.is_none());
        // And serializing a None filter omits the key entirely.
        let out = serde_json::to_string(&cfg).unwrap();
        assert!(!out.contains("tool_filter"));
    }

    #[test]
    fn test_health_status_serialization() {
        let healthy = HealthStatus::Healthy;
        let json = serde_json::to_string(&healthy).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));

        let degraded = HealthStatus::Degraded { failures: 3 };
        let json = serde_json::to_string(&degraded).unwrap();
        assert!(json.contains("\"status\":\"degraded\""));
        assert!(json.contains("\"failures\":3"));
    }
}

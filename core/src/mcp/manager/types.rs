//! MCP Manager Types
//!
//! Types for the McpManager actor, including commands, events, and server configuration.
//!
//! This module provides the foundational types for MCP orchestration:
//! - `McpManagerConfig` - Persistence-friendly server configuration
//! - `McpCommand` - Actor command enum for communication
//! - `McpManagerEvent` - Extended events for manager lifecycle
//! - Health tracking types for circuit breaker pattern

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::mcp::{McpClient, McpPrompt, McpResource, McpTool};

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
    /// Required runtime (e.g., "node", "python", "bun")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_runtime: Option<String>,
    /// Whether to auto-start this server
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// Request timeout in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

fn default_true() -> bool {
    true
}

impl McpManagerConfig {
    /// Create a new stdio server configuration
    pub fn stdio(id: impl Into<String>, name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            transport: McpTransportType::Stdio,
            command: Some(command.into()),
            args: Vec::new(),
            url: None,
            env: HashMap::new(),
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
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
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
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
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
        }
    }

    /// Set command arguments
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Set environment variables
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Set required runtime
    pub fn with_runtime(mut self, runtime: impl Into<String>) -> Self {
        self.requires_runtime = Some(runtime.into());
        self
    }

    /// Set auto-start flag
    pub fn with_auto_start(mut self, auto_start: bool) -> Self {
        self.auto_start = auto_start;
        self
    }

    /// Set timeout in seconds
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
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
    /// Number of prompts provided
    pub prompt_count: usize,
    /// Current health status
    pub health: HealthStatus,
}

/// Health status for circuit breaker pattern
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
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
    Stopped,
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self::Stopped
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded { failures } => write!(f, "degraded ({} failures)", failures),
            Self::Unhealthy => write!(f, "unhealthy"),
            Self::Restarting { attempt } => write!(f, "restarting (attempt {})", attempt),
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

    /// Record a failure
    pub fn record_failure(&mut self, error: impl Into<String>) {
        self.consecutive_failures += 1;
        self.last_check = Some(Instant::now());
        self.last_error = Some(error.into());

        // Update status based on failure count
        self.status = if self.consecutive_failures >= 5 {
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
    pub fn mark_dead(&mut self) {
        self.status = HealthStatus::Dead;
    }

    /// Mark as stopped
    pub fn mark_stopped(&mut self) {
        self.status = HealthStatus::Stopped;
        self.consecutive_failures = 0;
    }

    /// Check if server should be restarted
    pub fn should_restart(&self, max_restarts: u32, window_seconds: u64) -> bool {
        match self.status {
            HealthStatus::Unhealthy => {
                // Check if we're within restart window
                if let Some(start) = self.restart_window_start {
                    let elapsed = start.elapsed().as_secs();
                    if elapsed > window_seconds {
                        // Window expired, reset count
                        return true;
                    }
                    // Within window, check count
                    self.restart_count < max_restarts
                } else {
                    // No window yet, allow restart
                    true
                }
            }
            HealthStatus::Dead | HealthStatus::Stopped | HealthStatus::Restarting { .. } => false,
            _ => false,
        }
    }

    /// Reset restart window if expired
    pub fn maybe_reset_window(&mut self, window_seconds: u64) {
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

    /// Get the McpClient for a specific server
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
}

impl std::fmt::Debug for McpCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddServer { config, .. } => f
                .debug_struct("AddServer")
                .field("config", config)
                .finish(),
            Self::RemoveServer { server_id, .. } => f
                .debug_struct("RemoveServer")
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
            Self::GetStatus { server_id, .. } => f
                .debug_struct("GetStatus")
                .field("server_id", server_id)
                .finish(),
            Self::AggregateTools { .. } => f.debug_struct("AggregateTools").finish(),
            Self::AggregateResources { .. } => f.debug_struct("AggregateResources").finish(),
            Self::AggregatePrompts { .. } => f.debug_struct("AggregatePrompts").finish(),
            Self::ReloadConfig { .. } => f.debug_struct("ReloadConfig").finish(),
            Self::Shutdown { .. } => f.debug_struct("Shutdown").finish(),
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
    pub fn is_lifecycle_event(&self) -> bool {
        matches!(
            self,
            Self::ServerStarted { .. }
                | Self::ServerStopped { .. }
                | Self::ServerCrashed { .. }
                | Self::ServerRestarting { .. }
        )
    }

    /// Check if this is a capability change event
    pub fn is_capability_event(&self) -> bool {
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
        let config = McpManagerConfig::http("remote-id", "Remote Server", "https://api.example.com/mcp");
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
        let mut health = ServerHealth::default();
        health.consecutive_failures = 5;
        health.status = HealthStatus::Unhealthy;

        health.record_success();

        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.last_error.is_none());
    }

    #[test]
    fn test_server_health_record_failure() {
        let mut health = ServerHealth::default();
        health.status = HealthStatus::Healthy;

        // First failure - still healthy
        health.record_failure("error 1");
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.status, HealthStatus::Healthy);

        // Second failure - degraded
        health.record_failure("error 2");
        assert_eq!(health.consecutive_failures, 2);
        assert!(matches!(health.status, HealthStatus::Degraded { failures: 2 }));

        // More failures - unhealthy
        health.record_failure("error 3");
        health.record_failure("error 4");
        health.record_failure("error 5");
        assert_eq!(health.consecutive_failures, 5);
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_server_health_restarting() {
        let mut health = ServerHealth::default();

        health.mark_restarting();
        assert_eq!(health.restart_count, 1);
        assert!(matches!(health.status, HealthStatus::Restarting { attempt: 1 }));

        health.mark_restarting();
        assert_eq!(health.restart_count, 2);
        assert!(matches!(health.status, HealthStatus::Restarting { attempt: 2 }));
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

# MCP Orchestration Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the McpManager Actor and integrate MCP Resources/Prompts into the Agent Loop.

**Architecture:** Actor-based McpManager with control/data plane separation. Manager handles lifecycle (add/remove/restart servers), health checks, and config persistence. Data plane (tool calls) goes directly to `Arc<McpClient>`.

**Tech Stack:** Rust, Tokio (mpsc/oneshot/broadcast channels), serde_json, tokio::time

---

## Task 1: Create McpEvent and McpCommand Types

**Files:**
- Create: `core/src/mcp/manager/mod.rs`
- Create: `core/src/mcp/manager/types.rs`
- Modify: `core/src/mcp/mod.rs`

**Step 1: Create the manager module directory structure**

```bash
mkdir -p core/src/mcp/manager
```

**Step 2: Write the types module with McpCommand and extended McpEvent**

Create `core/src/mcp/manager/types.rs`:

```rust
//! MCP Manager Types
//!
//! Command and event types for the McpManager actor.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::Result;
use crate::mcp::client::McpClient;
use crate::mcp::prompts::McpPrompt;
use crate::mcp::types::{McpResource, McpTool};
use std::sync::Arc;

/// Server configuration for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique server identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Transport type: "stdio", "http", or "sse"
    pub transport: String,
    /// Command for stdio transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments for stdio transport
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// URL for http/sse transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Environment variables
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub env: std::collections::HashMap<String, String>,
    /// Required runtime (node, python, bun, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_runtime: Option<String>,
    /// Auto-start on manager initialization
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// Request timeout in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// Server information for listing
#[derive(Debug, Clone, Serialize)]
pub struct McpServerInfo {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub running: bool,
    pub tools_count: usize,
    pub resources_count: usize,
    pub prompts_count: usize,
}

/// Health status of a server
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded { failures: u32 },
    Unhealthy,
    Restarting { attempt: u32 },
    Dead,
    Stopped,
}

/// Server health tracking
#[derive(Debug, Clone)]
pub struct ServerHealth {
    pub consecutive_failures: u32,
    pub last_check: Instant,
    pub restart_count: u32,
    pub restart_window_start: Instant,
    pub status: HealthStatus,
    pub last_error: Option<String>,
}

impl Default for ServerHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_check: Instant::now(),
            restart_count: 0,
            restart_window_start: Instant::now(),
            status: HealthStatus::Stopped,
            last_error: None,
        }
    }
}

/// Detailed server status
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatusDetail {
    pub id: String,
    pub running: bool,
    pub health: HealthStatus,
    pub uptime_seconds: u64,
    pub restart_count: u32,
    pub last_error: Option<String>,
    pub tools_count: usize,
    pub resources_count: usize,
    pub prompts_count: usize,
}

/// Commands sent to the McpManager actor
pub enum McpCommand {
    // === Lifecycle ===
    AddServer(McpServerConfig, oneshot::Sender<Result<()>>),
    RemoveServer(String, oneshot::Sender<Result<()>>),
    RestartServer(String, oneshot::Sender<Result<()>>),
    StartServer(String, oneshot::Sender<Result<()>>),
    StopServer(String, oneshot::Sender<Result<()>>),

    // === Queries ===
    GetClient(String, oneshot::Sender<Option<Arc<McpClient>>>),
    ListServers(oneshot::Sender<Vec<McpServerInfo>>),
    GetStatus(String, oneshot::Sender<Option<McpServerStatusDetail>>),

    // === Aggregation (P1) ===
    AggregateTools(Option<String>, oneshot::Sender<Vec<McpTool>>),
    AggregateResources(Option<String>, oneshot::Sender<Vec<McpResource>>),
    AggregatePrompts(Option<String>, oneshot::Sender<Vec<McpPrompt>>),

    // === Config ===
    ReloadConfig(oneshot::Sender<Result<()>>),

    // === Control ===
    Shutdown,
}

/// Extended MCP events for the manager
#[derive(Debug, Clone)]
pub enum McpManagerEvent {
    /// Manager has completed initialization
    ManagerReady,
    /// Manager is shutting down
    ManagerShutdown,
    /// A server was added to config
    ServerAdded { id: String },
    /// A server was removed from config
    ServerRemoved { id: String },
    /// A server started successfully
    ServerStarted { id: String },
    /// A server stopped (gracefully)
    ServerStopped { id: String },
    /// A server crashed
    ServerCrashed { id: String, error: String },
    /// A server is restarting
    ServerRestarting { id: String, attempt: u32 },
    /// Tools list changed on a server
    ToolsChanged { server_id: String },
    /// Resources list changed on a server
    ResourcesChanged { server_id: String },
    /// Prompts list changed on a server
    PromptsChanged { server_id: String },
    /// Configuration was reloaded
    ConfigReloaded,
}

impl McpManagerEvent {
    /// Check if this event relates to a specific server
    pub fn relates_to(&self, server_id: &str) -> bool {
        match self {
            Self::ManagerReady | Self::ManagerShutdown | Self::ConfigReloaded => false,
            Self::ServerAdded { id }
            | Self::ServerRemoved { id }
            | Self::ServerStarted { id }
            | Self::ServerStopped { id }
            | Self::ServerCrashed { id, .. }
            | Self::ServerRestarting { id, .. }
            | Self::ToolsChanged { server_id: id }
            | Self::ResourcesChanged { server_id: id }
            | Self::PromptsChanged { server_id: id } => id == server_id,
        }
    }
}
```

**Step 3: Create the manager module entry point**

Create `core/src/mcp/manager/mod.rs`:

```rust
//! MCP Manager Module
//!
//! Actor-based management of MCP server lifecycle.

mod types;

pub use types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 4: Update mcp/mod.rs to export the manager module**

Add to `core/src/mcp/mod.rs` after line 51:

```rust
pub mod manager;

pub use manager::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 5: Run tests to verify compilation**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

Expected: Compilation succeeds

**Step 6: Commit**

```bash
git add core/src/mcp/manager/
git add core/src/mcp/mod.rs
git commit -m "feat(mcp): add McpCommand and McpManagerEvent types

Define command enum for actor communication and extended events
for manager lifecycle. Includes McpServerConfig for persistence.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement McpManagerHandle (Public API)

**Files:**
- Create: `core/src/mcp/manager/handle.rs`
- Modify: `core/src/mcp/manager/mod.rs`

**Step 1: Write the handle module**

Create `core/src/mcp/manager/handle.rs`:

```rust
//! McpManager Handle
//!
//! The public API for interacting with the McpManager actor.

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::Result;
use crate::mcp::client::McpClient;
use crate::mcp::prompts::McpPrompt;
use crate::mcp::types::{McpResource, McpTool};

use super::types::{
    McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo, McpServerStatusDetail,
};

/// Handle to the McpManager actor
///
/// This is the public interface for interacting with the manager.
/// It is cheap to clone and can be shared across tasks.
#[derive(Clone)]
pub struct McpManagerHandle {
    tx: mpsc::Sender<McpCommand>,
    event_tx: broadcast::Sender<McpManagerEvent>,
}

impl McpManagerHandle {
    /// Create a new handle (called internally by McpManagerActor)
    pub(crate) fn new(
        tx: mpsc::Sender<McpCommand>,
        event_tx: broadcast::Sender<McpManagerEvent>,
    ) -> Self {
        Self { tx, event_tx }
    }

    // === Lifecycle Methods ===

    /// Add a new MCP server
    pub async fn add_server(&self, config: McpServerConfig) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::AddServer(config, resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    /// Remove an MCP server
    pub async fn remove_server(&self, id: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::RemoveServer(id.to_string(), resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    /// Restart an MCP server
    pub async fn restart_server(&self, id: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::RestartServer(id.to_string(), resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    /// Start an MCP server
    pub async fn start_server(&self, id: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::StartServer(id.to_string(), resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    /// Stop an MCP server
    pub async fn stop_server(&self, id: &str) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::StopServer(id.to_string(), resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    // === Query Methods ===

    /// Get a client by server ID for direct data-plane operations
    pub async fn get_client(&self, id: &str) -> Option<Arc<McpClient>> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::GetClient(id.to_string(), resp_tx))
            .await
            .ok()?;
        resp_rx.await.ok()?
    }

    /// List all configured servers
    pub async fn list_servers(&self) -> Vec<McpServerInfo> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx.send(McpCommand::ListServers(resp_tx)).await.is_err() {
            return Vec::new();
        }
        resp_rx.await.unwrap_or_default()
    }

    /// Get detailed status for a specific server
    pub async fn get_status(&self, id: &str) -> Option<McpServerStatusDetail> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::GetStatus(id.to_string(), resp_tx))
            .await
            .ok()?;
        resp_rx.await.ok()?
    }

    // === Aggregation Methods (P1) ===

    /// Aggregate tools from all servers (or specific server)
    pub async fn aggregate_tools(&self, server_id: Option<&str>) -> Vec<McpTool> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self
            .tx
            .send(McpCommand::AggregateTools(
                server_id.map(String::from),
                resp_tx,
            ))
            .await
            .is_err()
        {
            return Vec::new();
        }
        resp_rx.await.unwrap_or_default()
    }

    /// Aggregate resources from all servers (or specific server)
    pub async fn aggregate_resources(&self, server_id: Option<&str>) -> Vec<McpResource> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self
            .tx
            .send(McpCommand::AggregateResources(
                server_id.map(String::from),
                resp_tx,
            ))
            .await
            .is_err()
        {
            return Vec::new();
        }
        resp_rx.await.unwrap_or_default()
    }

    /// Aggregate prompts from all servers (or specific server)
    pub async fn aggregate_prompts(&self, server_id: Option<&str>) -> Vec<McpPrompt> {
        let (resp_tx, resp_rx) = oneshot::channel();
        if self
            .tx
            .send(McpCommand::AggregatePrompts(
                server_id.map(String::from),
                resp_tx,
            ))
            .await
            .is_err()
        {
            return Vec::new();
        }
        resp_rx.await.unwrap_or_default()
    }

    // === Config Methods ===

    /// Reload configuration from disk
    pub async fn reload_config(&self) -> Result<()> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.tx
            .send(McpCommand::ReloadConfig(resp_tx))
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?;
        resp_rx
            .await
            .map_err(|_| crate::error::AetherError::ChannelClosed)?
    }

    /// Shutdown the manager
    pub async fn shutdown(&self) {
        let _ = self.tx.send(McpCommand::Shutdown).await;
    }

    // === Event Subscription ===

    /// Subscribe to manager events
    pub fn subscribe(&self) -> broadcast::Receiver<McpManagerEvent> {
        self.event_tx.subscribe()
    }

    /// Broadcast an event (used internally)
    pub(crate) fn broadcast(&self, event: McpManagerEvent) {
        let _ = self.event_tx.send(event);
    }
}
```

**Step 2: Update manager/mod.rs to export handle**

```rust
//! MCP Manager Module
//!
//! Actor-based management of MCP server lifecycle.

mod handle;
mod types;

pub use handle::McpManagerHandle;
pub use types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 3: Add ChannelClosed error variant if not exists**

Check `core/src/error.rs` and add if missing:

```rust
/// Channel closed error
#[error("Channel closed")]
ChannelClosed,
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

**Step 5: Commit**

```bash
git add core/src/mcp/manager/handle.rs
git add core/src/mcp/manager/mod.rs
git add core/src/error.rs
git commit -m "feat(mcp): add McpManagerHandle public API

Implement the public interface for McpManager with lifecycle,
query, aggregation, and event subscription methods.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Implement Config Persistence

**Files:**
- Create: `core/src/mcp/manager/config.rs`
- Modify: `core/src/mcp/manager/mod.rs`

**Step 1: Write the config persistence module**

Create `core/src/mcp/manager/config.rs`:

```rust
//! MCP Config Persistence
//!
//! Handles loading and saving MCP server configurations to disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::error::{AetherError, Result};

use super::types::McpServerConfig;

/// Persisted MCP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPersistentConfig {
    /// Config version for migration
    #[serde(default = "default_version")]
    pub version: u32,
    /// Server configurations by ID
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

fn default_version() -> u32 {
    1
}

impl Default for McpPersistentConfig {
    fn default() -> Self {
        Self {
            version: 1,
            servers: HashMap::new(),
        }
    }
}

impl McpPersistentConfig {
    /// Get the default config file path
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aether")
            .join("mcp_config.json")
    }

    /// Load config from a file path
    pub async fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            tracing::info!(path = %path.display(), "MCP config not found, using defaults");
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path).await.map_err(|e| {
            AetherError::ConfigError(format!("Failed to read MCP config: {}", e))
        })?;

        let config: Self = serde_json::from_str(&content).map_err(|e| {
            AetherError::ConfigError(format!("Failed to parse MCP config: {}", e))
        })?;

        tracing::info!(
            path = %path.display(),
            server_count = config.servers.len(),
            "Loaded MCP config"
        );

        Ok(config)
    }

    /// Save config to a file path
    pub async fn save(&self, path: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                AetherError::ConfigError(format!("Failed to create config directory: {}", e))
            })?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|e| {
            AetherError::ConfigError(format!("Failed to serialize MCP config: {}", e))
        })?;

        fs::write(path, content).await.map_err(|e| {
            AetherError::ConfigError(format!("Failed to write MCP config: {}", e))
        })?;

        tracing::info!(
            path = %path.display(),
            server_count = self.servers.len(),
            "Saved MCP config"
        );

        Ok(())
    }

    /// Add or update a server config
    pub fn upsert_server(&mut self, config: McpServerConfig) {
        self.servers.insert(config.id.clone(), config);
    }

    /// Remove a server config
    pub fn remove_server(&mut self, id: &str) -> Option<McpServerConfig> {
        self.servers.remove(id)
    }

    /// Get a server config by ID
    pub fn get_server(&self, id: &str) -> Option<&McpServerConfig> {
        self.servers.get(id)
    }

    /// Get servers that should auto-start
    pub fn auto_start_servers(&self) -> Vec<&McpServerConfig> {
        self.servers.values().filter(|s| s.auto_start).collect()
    }

    /// Expand environment variables in config values
    pub fn expand_env_vars(&mut self) {
        for server in self.servers.values_mut() {
            // Expand in env vars
            for value in server.env.values_mut() {
                if let Some(expanded) = expand_env_var(value) {
                    *value = expanded;
                }
            }
            // Expand in command
            if let Some(cmd) = &mut server.command {
                if let Some(expanded) = expand_env_var(cmd) {
                    *cmd = expanded;
                }
            }
            // Expand in args
            for arg in &mut server.args {
                if let Some(expanded) = expand_env_var(arg) {
                    *arg = expanded;
                }
            }
        }
    }
}

/// Expand ${VAR} patterns in a string
fn expand_env_var(s: &str) -> Option<String> {
    if !s.contains("${") {
        return None;
    }

    let mut result = s.to_string();
    let re = regex::Regex::new(r"\$\{([^}]+)\}").ok()?;

    for cap in re.captures_iter(s) {
        let var_name = &cap[1];
        if let Ok(value) = std::env::var(var_name) {
            result = result.replace(&cap[0], &value);
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_config_default() {
        let config = McpPersistentConfig::default();
        assert_eq!(config.version, 1);
        assert!(config.servers.is_empty());
    }

    #[tokio::test]
    async fn test_config_save_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp_config.json");

        let mut config = McpPersistentConfig::default();
        config.upsert_server(McpServerConfig {
            id: "test-server".to_string(),
            name: "Test Server".to_string(),
            transport: "stdio".to_string(),
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            url: None,
            env: HashMap::new(),
            requires_runtime: Some("node".to_string()),
            auto_start: true,
            timeout_seconds: Some(30),
        });

        config.save(&path).await.unwrap();
        assert!(path.exists());

        let loaded = McpPersistentConfig::load(&path).await.unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert!(loaded.servers.contains_key("test-server"));
    }

    #[tokio::test]
    async fn test_config_load_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");

        let config = McpPersistentConfig::load(&path).await.unwrap();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_expand_env_var() {
        std::env::set_var("TEST_VAR", "test_value");
        let result = expand_env_var("prefix_${TEST_VAR}_suffix");
        assert_eq!(result, Some("prefix_test_value_suffix".to_string()));

        let result = expand_env_var("no_var_here");
        assert_eq!(result, None);
    }

    #[test]
    fn test_auto_start_servers() {
        let mut config = McpPersistentConfig::default();
        config.upsert_server(McpServerConfig {
            id: "auto".to_string(),
            name: "Auto".to_string(),
            transport: "stdio".to_string(),
            command: Some("node".to_string()),
            args: vec![],
            url: None,
            env: HashMap::new(),
            requires_runtime: None,
            auto_start: true,
            timeout_seconds: None,
        });
        config.upsert_server(McpServerConfig {
            id: "manual".to_string(),
            name: "Manual".to_string(),
            transport: "stdio".to_string(),
            command: Some("node".to_string()),
            args: vec![],
            url: None,
            env: HashMap::new(),
            requires_runtime: None,
            auto_start: false,
            timeout_seconds: None,
        });

        let auto_servers = config.auto_start_servers();
        assert_eq!(auto_servers.len(), 1);
        assert_eq!(auto_servers[0].id, "auto");
    }
}
```

**Step 2: Add regex to Cargo.toml if not present**

Check `core/Cargo.toml` and add if missing:

```toml
regex = "1"
```

**Step 3: Update manager/mod.rs**

```rust
//! MCP Manager Module
//!
//! Actor-based management of MCP server lifecycle.

mod config;
mod handle;
mod types;

pub use config::McpPersistentConfig;
pub use handle::McpManagerHandle;
pub use types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp::manager::config
```

**Step 5: Commit**

```bash
git add core/src/mcp/manager/config.rs
git add core/src/mcp/manager/mod.rs
git add core/Cargo.toml
git commit -m "feat(mcp): add config persistence for McpManager

Implement McpPersistentConfig with JSON serialization,
environment variable expansion, and auto-start filtering.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Implement McpManagerActor Core Loop

**Files:**
- Create: `core/src/mcp/manager/actor.rs`
- Modify: `core/src/mcp/manager/mod.rs`

**Step 1: Write the actor module**

Create `core/src/mcp/manager/actor.rs`:

```rust
//! MCP Manager Actor
//!
//! The actor that runs the command loop and manages server lifecycle.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

use crate::error::{AetherError, Result};
use crate::mcp::client::{ExternalServerConfig, McpClient};
use crate::mcp::prompts::McpPrompt;
use crate::mcp::types::{McpResource, McpTool};

use super::config::McpPersistentConfig;
use super::handle::McpManagerHandle;
use super::types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Interval between health checks
    pub interval: Duration,
    /// Timeout for each health check
    pub timeout: Duration,
    /// Number of consecutive failures before marking unhealthy
    pub max_failures: u32,
    /// Delay before attempting restart
    pub restart_delay: Duration,
    /// Maximum restarts within the restart window
    pub max_restarts: u32,
    /// Window for counting restarts (e.g., 5 minutes)
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

/// MCP Manager Actor internal state
pub struct McpManagerActor {
    /// Path to config file
    config_path: PathBuf,
    /// Persisted configuration
    config: McpPersistentConfig,
    /// Active clients by server ID
    clients: HashMap<String, Arc<McpClient>>,
    /// Health state by server ID
    health_states: HashMap<String, ServerHealth>,
    /// Server start times for uptime calculation
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
    /// Create a new actor and return its handle
    pub async fn new(config_path: Option<PathBuf>) -> Result<(Self, McpManagerHandle)> {
        let config_path = config_path.unwrap_or_else(McpPersistentConfig::default_path);
        let mut config = McpPersistentConfig::load(&config_path).await?;
        config.expand_env_vars();

        let (cmd_tx, cmd_rx) = mpsc::channel(256);
        let (event_tx, _) = broadcast::channel(128);

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

    /// Run the actor's main loop
    pub async fn run(mut self) {
        tracing::info!("McpManager starting");

        // Auto-start servers
        self.auto_start_servers().await;

        // Broadcast ready event
        self.broadcast(McpManagerEvent::ManagerReady);

        // Main command loop
        loop {
            tokio::select! {
                Some(cmd) = self.cmd_rx.recv() => {
                    if !self.handle_command(cmd).await {
                        break;
                    }
                }
                else => break,
            }
        }

        // Cleanup
        self.shutdown_all().await;
        self.broadcast(McpManagerEvent::ManagerShutdown);
        tracing::info!("McpManager stopped");
    }

    /// Handle a single command, returns false if should shutdown
    async fn handle_command(&mut self, cmd: McpCommand) -> bool {
        match cmd {
            McpCommand::AddServer(config, resp) => {
                let result = self.add_server(config).await;
                let _ = resp.send(result);
            }
            McpCommand::RemoveServer(id, resp) => {
                let result = self.remove_server(&id).await;
                let _ = resp.send(result);
            }
            McpCommand::RestartServer(id, resp) => {
                let result = self.restart_server(&id).await;
                let _ = resp.send(result);
            }
            McpCommand::StartServer(id, resp) => {
                let result = self.start_server(&id).await;
                let _ = resp.send(result);
            }
            McpCommand::StopServer(id, resp) => {
                let result = self.stop_server(&id).await;
                let _ = resp.send(result);
            }
            McpCommand::GetClient(id, resp) => {
                let client = self.clients.get(&id).cloned();
                let _ = resp.send(client);
            }
            McpCommand::ListServers(resp) => {
                let servers = self.list_servers().await;
                let _ = resp.send(servers);
            }
            McpCommand::GetStatus(id, resp) => {
                let status = self.get_status(&id).await;
                let _ = resp.send(status);
            }
            McpCommand::AggregateTools(server_id, resp) => {
                let tools = self.aggregate_tools(server_id.as_deref()).await;
                let _ = resp.send(tools);
            }
            McpCommand::AggregateResources(server_id, resp) => {
                let resources = self.aggregate_resources(server_id.as_deref()).await;
                let _ = resp.send(resources);
            }
            McpCommand::AggregatePrompts(server_id, resp) => {
                let prompts = self.aggregate_prompts(server_id.as_deref()).await;
                let _ = resp.send(prompts);
            }
            McpCommand::ReloadConfig(resp) => {
                let result = self.reload_config().await;
                let _ = resp.send(result);
            }
            McpCommand::Shutdown => {
                return false;
            }
        }
        true
    }

    /// Auto-start servers marked for auto_start
    async fn auto_start_servers(&mut self) {
        let configs: Vec<_> = self.config.auto_start_servers().cloned().collect();
        tracing::info!(count = configs.len(), "Auto-starting MCP servers");

        for config in configs {
            if let Err(e) = self.start_server_internal(&config).await {
                tracing::error!(
                    server = %config.id,
                    error = %e,
                    "Failed to auto-start MCP server"
                );
            }
        }
    }

    /// Add a new server (persists config)
    async fn add_server(&mut self, config: McpServerConfig) -> Result<()> {
        let id = config.id.clone();

        // Add to config and save
        self.config.upsert_server(config.clone());
        self.config.save(&self.config_path).await?;

        // Start if auto_start
        if config.auto_start {
            self.start_server_internal(&config).await?;
        }

        self.broadcast(McpManagerEvent::ServerAdded { id });
        Ok(())
    }

    /// Remove a server (persists config)
    async fn remove_server(&mut self, id: &str) -> Result<()> {
        // Stop if running
        self.stop_server_internal(id).await;

        // Remove from config and save
        self.config.remove_server(id);
        self.config.save(&self.config_path).await?;

        self.broadcast(McpManagerEvent::ServerRemoved { id: id.to_string() });
        Ok(())
    }

    /// Restart a server
    async fn restart_server(&mut self, id: &str) -> Result<()> {
        let config = self.config.get_server(id).cloned().ok_or_else(|| {
            AetherError::NotFound(format!("MCP server not found: {}", id))
        })?;

        self.stop_server_internal(id).await;
        tokio::time::sleep(self.health_config.restart_delay).await;
        self.start_server_internal(&config).await?;

        Ok(())
    }

    /// Start a server by ID
    async fn start_server(&mut self, id: &str) -> Result<()> {
        let config = self.config.get_server(id).cloned().ok_or_else(|| {
            AetherError::NotFound(format!("MCP server not found: {}", id))
        })?;

        self.start_server_internal(&config).await
    }

    /// Stop a server by ID
    async fn stop_server(&mut self, id: &str) -> Result<()> {
        self.stop_server_internal(id).await;
        Ok(())
    }

    /// Internal: Start a server from config
    async fn start_server_internal(&mut self, config: &McpServerConfig) -> Result<()> {
        let id = &config.id;

        // Skip if already running
        if self.clients.contains_key(id) {
            tracing::debug!(server = %id, "Server already running");
            return Ok(());
        }

        tracing::info!(server = %id, transport = %config.transport, "Starting MCP server");

        // Create client based on transport
        let client = McpClient::new();

        match config.transport.as_str() {
            "stdio" => {
                let command = config.command.as_ref().ok_or_else(|| {
                    AetherError::ConfigError(format!("Missing command for stdio server: {}", id))
                })?;

                let ext_config = ExternalServerConfig {
                    name: id.clone(),
                    command: command.clone(),
                    args: config.args.clone(),
                    env: config.env.clone(),
                    cwd: None,
                    requires_runtime: config.requires_runtime.clone(),
                    timeout_seconds: config.timeout_seconds,
                };

                client.start_external_server(ext_config).await?;
            }
            "http" | "sse" => {
                let url = config.url.as_ref().ok_or_else(|| {
                    AetherError::ConfigError(format!("Missing URL for HTTP/SSE server: {}", id))
                })?;

                let transport = match config.transport.as_str() {
                    "sse" => crate::mcp::types::TransportPreference::Sse,
                    _ => crate::mcp::types::TransportPreference::Http,
                };

                let remote_config = crate::mcp::types::McpRemoteServerConfig::new(id, url)
                    .with_transport(transport)
                    .with_timeout(config.timeout_seconds.unwrap_or(300));

                client.start_remote_server(remote_config).await?;
            }
            other => {
                return Err(AetherError::ConfigError(format!(
                    "Unknown transport type: {}",
                    other
                )));
            }
        }

        // Store client and health state
        self.clients.insert(id.clone(), Arc::new(client));
        self.health_states.insert(id.clone(), ServerHealth {
            status: HealthStatus::Healthy,
            ..Default::default()
        });
        self.start_times.insert(id.clone(), Instant::now());

        self.broadcast(McpManagerEvent::ServerStarted { id: id.clone() });
        Ok(())
    }

    /// Internal: Stop a server
    async fn stop_server_internal(&mut self, id: &str) {
        if let Some(client) = self.clients.remove(id) {
            tracing::info!(server = %id, "Stopping MCP server");
            if let Err(e) = client.stop_all().await {
                tracing::warn!(server = %id, error = %e, "Error stopping server");
            }
        }
        self.health_states.remove(id);
        self.start_times.remove(id);
        self.broadcast(McpManagerEvent::ServerStopped { id: id.to_string() });
    }

    /// Shutdown all servers
    async fn shutdown_all(&mut self) {
        let ids: Vec<_> = self.clients.keys().cloned().collect();
        for id in ids {
            self.stop_server_internal(&id).await;
        }
    }

    /// List all servers with status
    async fn list_servers(&self) -> Vec<McpServerInfo> {
        let mut result = Vec::new();

        for (id, config) in &self.config.servers {
            let running = self.clients.contains_key(id);
            let (tools_count, resources_count, prompts_count) = if running {
                if let Some(client) = self.clients.get(id) {
                    (client.list_tools().await.len(), 0, 0) // TODO: resources/prompts count
                } else {
                    (0, 0, 0)
                }
            } else {
                (0, 0, 0)
            };

            result.push(McpServerInfo {
                id: id.clone(),
                name: config.name.clone(),
                transport: config.transport.clone(),
                running,
                tools_count,
                resources_count,
                prompts_count,
            });
        }

        result
    }

    /// Get detailed status for a server
    async fn get_status(&self, id: &str) -> Option<McpServerStatusDetail> {
        let config = self.config.get_server(id)?;
        let health = self.health_states.get(id).cloned().unwrap_or_default();
        let running = self.clients.contains_key(id);
        let uptime = self.start_times.get(id).map(|t| t.elapsed().as_secs()).unwrap_or(0);

        let (tools_count, resources_count, prompts_count) = if let Some(client) = self.clients.get(id) {
            (client.list_tools().await.len(), 0, 0)
        } else {
            (0, 0, 0)
        };

        Some(McpServerStatusDetail {
            id: id.to_string(),
            running,
            health: health.status,
            uptime_seconds: uptime,
            restart_count: health.restart_count,
            last_error: health.last_error,
            tools_count,
            resources_count,
            prompts_count,
        })
    }

    /// Aggregate tools from servers
    async fn aggregate_tools(&self, server_id: Option<&str>) -> Vec<McpTool> {
        let mut tools = Vec::new();

        let clients: Vec<_> = if let Some(id) = server_id {
            self.clients.get(id).cloned().into_iter().collect()
        } else {
            self.clients.values().cloned().collect()
        };

        for client in clients {
            tools.extend(client.list_tools().await);
        }

        tools
    }

    /// Aggregate resources from servers (P1 - stub)
    async fn aggregate_resources(&self, _server_id: Option<&str>) -> Vec<McpResource> {
        // TODO: Implement when McpServerConnection supports resources/list
        Vec::new()
    }

    /// Aggregate prompts from servers (P1 - stub)
    async fn aggregate_prompts(&self, _server_id: Option<&str>) -> Vec<McpPrompt> {
        // TODO: Implement when McpServerConnection supports prompts/list
        Vec::new()
    }

    /// Reload configuration from disk
    async fn reload_config(&mut self) -> Result<()> {
        let mut new_config = McpPersistentConfig::load(&self.config_path).await?;
        new_config.expand_env_vars();

        // Find servers to stop (removed from config)
        let current_ids: std::collections::HashSet<_> = self.config.servers.keys().collect();
        let new_ids: std::collections::HashSet<_> = new_config.servers.keys().collect();

        for id in current_ids.difference(&new_ids) {
            self.stop_server_internal(id).await;
        }

        // Update config
        self.config = new_config;

        // Start new auto-start servers
        for config in self.config.auto_start_servers().cloned().collect::<Vec<_>>() {
            if !self.clients.contains_key(&config.id) {
                if let Err(e) = self.start_server_internal(&config).await {
                    tracing::error!(server = %config.id, error = %e, "Failed to start new server");
                }
            }
        }

        self.broadcast(McpManagerEvent::ConfigReloaded);
        Ok(())
    }

    /// Broadcast an event
    fn broadcast(&self, event: McpManagerEvent) {
        let _ = self.event_tx.send(event);
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

        let (actor, _handle) = McpManagerActor::new(Some(config_path)).await.unwrap();
        assert!(actor.clients.is_empty());
    }

    #[tokio::test]
    async fn test_health_check_config_default() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.interval, Duration::from_secs(30));
        assert_eq!(config.max_failures, 3);
    }
}
```

**Step 2: Update manager/mod.rs**

```rust
//! MCP Manager Module
//!
//! Actor-based management of MCP server lifecycle.

mod actor;
mod config;
mod handle;
mod types;

pub use actor::{HealthCheckConfig, McpManagerActor};
pub use config::McpPersistentConfig;
pub use handle::McpManagerHandle;
pub use types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 3: Add NotFound error variant if not exists**

Check `core/src/error.rs`:

```rust
/// Resource not found
#[error("Not found: {0}")]
NotFound(String),
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp::manager
```

**Step 5: Commit**

```bash
git add core/src/mcp/manager/actor.rs
git add core/src/mcp/manager/mod.rs
git add core/src/error.rs
git commit -m "feat(mcp): implement McpManagerActor core loop

Add the actor that handles command processing, server lifecycle,
config persistence, and event broadcasting. Includes auto-start
and graceful shutdown.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Wire Gateway Handlers

**Files:**
- Modify: `core/src/gateway/handlers/mcp.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Rewrite mcp.rs handlers to use McpManagerHandle**

Replace `core/src/gateway/handlers/mcp.rs`:

```rust
//! MCP Server RPC Handlers
//!
//! Handlers for MCP server management via McpManager.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::mcp::manager::{McpManagerHandle, McpServerConfig, McpServerInfo, McpServerStatusDetail};

// Re-export for backwards compatibility
pub use crate::mcp::manager::McpServerInfo as McpServerInfoExport;

/// Create handlers with McpManagerHandle dependency
pub fn create_handlers(
    handle: McpManagerHandle,
) -> impl Fn(&str) -> Option<Box<dyn Fn(JsonRpcRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = JsonRpcResponse> + Send>> + Send + Sync>> {
    move |method: &str| -> Option<Box<dyn Fn(JsonRpcRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = JsonRpcResponse> + Send>> + Send + Sync>> {
        let h = handle.clone();
        match method {
            "mcp.list" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_list(req, handle).await })
            })),
            "mcp.add" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_add(req, handle).await })
            })),
            "mcp.delete" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_delete(req, handle).await })
            })),
            "mcp.status" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_status(req, handle).await })
            })),
            "mcp.start" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_start(req, handle).await })
            })),
            "mcp.stop" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_stop(req, handle).await })
            })),
            "mcp.restart" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_restart(req, handle).await })
            })),
            "mcp.listTools" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_list_tools(req, handle).await })
            })),
            "mcp.listResources" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_list_resources(req, handle).await })
            })),
            "mcp.listPrompts" => Some(Box::new(move |req| {
                let handle = h.clone();
                Box::pin(async move { handle_list_prompts(req, handle).await })
            })),
            _ => None,
        }
    }
}

// ============================================================================
// List
// ============================================================================

/// List all MCP servers
pub async fn handle_list(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let servers = handle.list_servers().await;
    JsonRpcResponse::success(request.id, json!({ "servers": servers }))
}

// ============================================================================
// Add
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AddParams {
    pub config: McpServerConfig,
}

/// Add a new MCP server
pub async fn handle_add(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: AddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.add_server(params.config).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Delete
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

/// Delete an MCP server
pub async fn handle_delete(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.remove_server(&params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Status
// ============================================================================

/// Get MCP server status
pub async fn handle_status(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.get_status(&params.id).await {
        Some(status) => JsonRpcResponse::success(request.id, serde_json::to_value(status).unwrap()),
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Server not found: {}", params.id),
        ),
    }
}

// ============================================================================
// Start/Stop/Restart
// ============================================================================

/// Start an MCP server
pub async fn handle_start(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.start_server(&params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Stop an MCP server
pub async fn handle_stop(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.stop_server(&params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Restart an MCP server
pub async fn handle_restart(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: IdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match handle.restart_server(&params.id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Aggregation (P1)
// ============================================================================

#[derive(Debug, Deserialize, Default)]
pub struct AggregateParams {
    #[serde(default)]
    pub server_id: Option<String>,
}

/// List tools from all servers (or specific server)
pub async fn handle_list_tools(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: AggregateParams = parse_params(&request).unwrap_or_default();
    let tools = handle.aggregate_tools(params.server_id.as_deref()).await;
    JsonRpcResponse::success(request.id, json!({ "tools": tools }))
}

/// List resources from all servers (or specific server)
pub async fn handle_list_resources(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: AggregateParams = parse_params(&request).unwrap_or_default();
    let resources = handle.aggregate_resources(params.server_id.as_deref()).await;
    JsonRpcResponse::success(request.id, json!({ "resources": resources }))
}

/// List prompts from all servers (or specific server)
pub async fn handle_list_prompts(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    let params: AggregateParams = parse_params(&request).unwrap_or_default();
    let prompts = handle.aggregate_prompts(params.server_id.as_deref()).await;
    JsonRpcResponse::success(request.id, json!({ "prompts": prompts }))
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_params<T: serde::de::DeserializeOwned>(request: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(request.id.clone(), INVALID_PARAMS, format!("Invalid params: {}", e))
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_params_deserialize() {
        let json = serde_json::json!({
            "config": {
                "id": "test",
                "name": "Test",
                "transport": "stdio",
                "command": "node",
                "args": ["server.js"]
            }
        });
        let params: AddParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.config.id, "test");
    }
}
```

**Step 2: Add handler registration helper to HandlerRegistry**

Add method to `core/src/gateway/handlers/mod.rs` after line 276:

```rust
    /// Register MCP handlers with a manager handle
    pub fn register_mcp_handlers(&mut self, handle: crate::mcp::manager::McpManagerHandle) {
        let h = handle.clone();
        self.register("mcp.list", move |req| {
            let handle = h.clone();
            async move { mcp::handle_list(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.add", move |req| {
            let handle = h.clone();
            async move { mcp::handle_add(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.delete", move |req| {
            let handle = h.clone();
            async move { mcp::handle_delete(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.status", move |req| {
            let handle = h.clone();
            async move { mcp::handle_status(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.start", move |req| {
            let handle = h.clone();
            async move { mcp::handle_start(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.stop", move |req| {
            let handle = h.clone();
            async move { mcp::handle_stop(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.restart", move |req| {
            let handle = h.clone();
            async move { mcp::handle_restart(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.listTools", move |req| {
            let handle = h.clone();
            async move { mcp::handle_list_tools(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.listResources", move |req| {
            let handle = h.clone();
            async move { mcp::handle_list_resources(req, handle).await }
        });

        let h = handle.clone();
        self.register("mcp.listPrompts", move |req| {
            let handle = h.clone();
            async move { mcp::handle_list_prompts(req, handle).await }
        });
    }
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/mcp.rs
git add core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): wire MCP handlers to McpManagerHandle

Replace stub handlers with real implementations that delegate
to the McpManager actor. Add register_mcp_handlers() helper.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Update mcp/mod.rs Exports

**Files:**
- Modify: `core/src/mcp/mod.rs`

**Step 1: Update exports to include all manager types**

Replace the pub use section at the end of `core/src/mcp/mod.rs`:

```rust
pub mod manager;

pub use manager::{
    HealthCheckConfig, HealthStatus, McpCommand, McpManagerActor, McpManagerEvent,
    McpManagerHandle, McpPersistentConfig, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 2: Run full test suite**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test
```

**Step 3: Commit**

```bash
git add core/src/mcp/mod.rs
git commit -m "feat(mcp): export all manager types from mcp module

Make McpManagerActor, McpManagerHandle, and related types
available via the mcp module for gateway integration.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Add Health Check Loop (P0.4)

**Files:**
- Create: `core/src/mcp/manager/health.rs`
- Modify: `core/src/mcp/manager/actor.rs`
- Modify: `core/src/mcp/manager/mod.rs`

**Step 1: Create health check module**

Create `core/src/mcp/manager/health.rs`:

```rust
//! Health Check Logic
//!
//! Background task for monitoring MCP server health.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

use crate::mcp::client::McpClient;

use super::types::{HealthStatus, McpManagerEvent, ServerHealth};
use super::HealthCheckConfig;

/// Health check result for a single server
pub struct HealthCheckResult {
    pub server_id: String,
    pub healthy: bool,
    pub error: Option<String>,
}

/// Run health check on a single client
pub async fn check_client_health(
    server_id: &str,
    client: &Arc<McpClient>,
    timeout: Duration,
) -> HealthCheckResult {
    let result = tokio::time::timeout(timeout, client.list_tools()).await;

    match result {
        Ok(tools) => {
            // If we can list tools, the server is healthy
            tracing::trace!(
                server = %server_id,
                tool_count = tools.len(),
                "Health check passed"
            );
            HealthCheckResult {
                server_id: server_id.to_string(),
                healthy: true,
                error: None,
            }
        }
        Err(_) => {
            tracing::warn!(server = %server_id, "Health check timed out");
            HealthCheckResult {
                server_id: server_id.to_string(),
                healthy: false,
                error: Some("Health check timed out".to_string()),
            }
        }
    }
}

/// Update health state based on check result
pub fn update_health_state(
    state: &mut ServerHealth,
    result: &HealthCheckResult,
    config: &HealthCheckConfig,
) -> Option<McpManagerEvent> {
    state.last_check = Instant::now();

    if result.healthy {
        state.consecutive_failures = 0;
        state.status = HealthStatus::Healthy;
        state.last_error = None;
        return None;
    }

    // Unhealthy result
    state.consecutive_failures += 1;
    state.last_error = result.error.clone();

    if state.consecutive_failures >= config.max_failures {
        // Check restart window
        if state.restart_window_start.elapsed() > config.restart_window {
            state.restart_count = 0;
            state.restart_window_start = Instant::now();
        }

        if state.restart_count < config.max_restarts {
            state.restart_count += 1;
            state.status = HealthStatus::Restarting {
                attempt: state.restart_count,
            };
            return Some(McpManagerEvent::ServerRestarting {
                id: result.server_id.clone(),
                attempt: state.restart_count,
            });
        } else {
            state.status = HealthStatus::Dead;
            return Some(McpManagerEvent::ServerCrashed {
                id: result.server_id.clone(),
                error: "Max restart attempts exceeded".to_string(),
            });
        }
    } else {
        state.status = HealthStatus::Degraded {
            failures: state.consecutive_failures,
        };
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_health_state_healthy() {
        let mut state = ServerHealth::default();
        state.consecutive_failures = 2;
        state.status = HealthStatus::Degraded { failures: 2 };

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            healthy: true,
            error: None,
        };

        let config = HealthCheckConfig::default();
        let event = update_health_state(&mut state, &result, &config);

        assert!(event.is_none());
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_update_health_state_degraded() {
        let mut state = ServerHealth::default();

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        let config = HealthCheckConfig::default();
        let event = update_health_state(&mut state, &result, &config);

        assert!(event.is_none());
        assert_eq!(state.consecutive_failures, 1);
        assert!(matches!(state.status, HealthStatus::Degraded { failures: 1 }));
    }

    #[test]
    fn test_update_health_state_needs_restart() {
        let mut state = ServerHealth::default();
        state.consecutive_failures = 2; // Will become 3

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        let config = HealthCheckConfig {
            max_failures: 3,
            ..Default::default()
        };
        let event = update_health_state(&mut state, &result, &config);

        assert!(matches!(event, Some(McpManagerEvent::ServerRestarting { .. })));
        assert!(matches!(state.status, HealthStatus::Restarting { attempt: 1 }));
    }
}
```

**Step 2: Update mod.rs**

```rust
//! MCP Manager Module
//!
//! Actor-based management of MCP server lifecycle.

mod actor;
mod config;
mod handle;
mod health;
mod types;

pub use actor::{HealthCheckConfig, McpManagerActor};
pub use config::McpPersistentConfig;
pub use handle::McpManagerHandle;
pub use types::{
    HealthStatus, McpCommand, McpManagerEvent, McpServerConfig, McpServerInfo,
    McpServerStatusDetail, ServerHealth,
};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp::manager::health
```

**Step 4: Commit**

```bash
git add core/src/mcp/manager/health.rs
git add core/src/mcp/manager/mod.rs
git commit -m "feat(mcp): add health check logic for servers

Implement health check utilities with degradation detection
and restart triggering based on consecutive failures.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

This implementation plan covers **P0 (basic infrastructure)** with:

1. **Task 1**: McpCommand and McpManagerEvent types
2. **Task 2**: McpManagerHandle public API
3. **Task 3**: Config persistence (JSON)
4. **Task 4**: McpManagerActor core loop
5. **Task 5**: Gateway handler wiring
6. **Task 6**: Module exports
7. **Task 7**: Health check logic

**P1 tasks (Resources/Prompts injection)** will be in a follow-up plan after P0 is stable, as they depend on:
- `McpServerConnection` supporting `resources/list` and `prompts/list`
- PromptBuilder integration

**Testing Strategy:**
- Each task includes unit tests
- Integration test after Task 5: Start Gateway with McpManager, call RPC methods
- Manual test: Add an MCP server config, restart Aether, verify server auto-starts

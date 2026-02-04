# Phase 1: Daemon Manager Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the foundational infrastructure for Aether's proactive AI system: Daemon Manager, IPC Channel, and Resource Governor.

**Architecture:** This phase creates the backbone that allows Aether to run as a persistent system service. The Daemon Manager provides cross-platform service management (starting with macOS launchd), the IPC Channel enables CLI-daemon communication via Unix Domain Socket with JSON-RPC 2.0 protocol, and the Resource Governor ensures frugal resource usage.

**Tech Stack:**
- Rust + Tokio for async daemon
- `plist` crate for macOS launchd configuration
- `sysinfo` for process/system monitoring
- `battery` for power status
- Unix Domain Socket for IPC
- JSON-RPC 2.0 for protocol

**Related Documents:**
- Design: `docs/plans/2026-02-04-proactive-ai-architecture-design.md`
- Architecture: `docs/ARCHITECTURE.md`

---

## Task 1: Create Daemon Module Foundation

**Files:**
- Create: `core/src/daemon/mod.rs`
- Create: `core/src/daemon/types.rs`
- Create: `core/src/daemon/error.rs`
- Modify: `core/src/lib.rs`
- Create: `core/src/daemon/tests/mod.rs`

### Step 1: Write the failing test for daemon module initialization

```rust
// core/src/daemon/tests/mod.rs
#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_daemon_module_exists() {
        // This test ensures the daemon module is properly declared
        let config = DaemonConfig::default();
        assert_eq!(config.socket_path, "~/.aether/daemon.sock");
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test --lib test_daemon_module_exists`
Expected: FAIL with "unresolved import" or "cannot find type `DaemonConfig`"

### Step 3: Create daemon module foundation

```rust
// core/src/daemon/mod.rs
//! Daemon Manager
//!
//! Manages Aether as a persistent system service across platforms.

pub mod error;
pub mod types;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};

/// Initialize daemon subsystem
pub fn init() -> Result<()> {
    Ok(())
}
```

```rust
// core/src/daemon/types.rs
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Daemon configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Unix Domain Socket path for IPC
    pub socket_path: String,

    /// Binary path for daemon executable
    pub binary_path: PathBuf,

    /// Log directory
    pub log_dir: PathBuf,

    /// Nice value (process priority, 10 = low priority)
    pub nice_value: i32,

    /// Soft memory limit in bytes (512MB)
    pub soft_mem_limit: u64,

    /// Hard memory limit in bytes (1GB)
    pub hard_mem_limit: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: "~/.aether/daemon.sock".to_string(),
            binary_path: PathBuf::from("~/.aether/bin/aether-daemon"),
            log_dir: PathBuf::from("~/.aether/logs"),
            nice_value: 10,
            soft_mem_limit: 512 * 1024 * 1024,  // 512MB
            hard_mem_limit: 1024 * 1024 * 1024, // 1GB
        }
    }
}

/// Daemon runtime status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonStatus {
    Running,
    Stopped,
    Unknown,
}

/// Service installation status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceStatus {
    Installed,
    NotInstalled,
    Unknown,
}
```

```rust
// core/src/daemon/error.rs
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DaemonError>;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("Service operation failed: {0}")]
    ServiceError(String),

    #[error("IPC error: {0}")]
    IpcError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Resource governor error: {0}")]
    ResourceGovernor(String),
}
```

### Step 4: Add daemon module to lib.rs

```rust
// core/src/lib.rs (add after line 88)
pub mod daemon;
```

### Step 5: Run test to verify it passes

Run: `cargo test --lib test_daemon_module_exists`
Expected: PASS

### Step 6: Commit

```bash
git add core/src/daemon/
git add core/src/lib.rs
git commit -m "daemon: create module foundation with types and error handling"
```

---

## Task 2: Implement ServiceManager Trait

**Files:**
- Create: `core/src/daemon/service_manager.rs`
- Modify: `core/src/daemon/mod.rs`
- Create: `core/src/daemon/tests/service_manager_tests.rs`

### Step 1: Write the failing test for ServiceManager trait

```rust
// core/src/daemon/tests/service_manager_tests.rs
#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_service_manager_trait_exists() {
        // Mock implementation to test trait
        struct MockService;

        #[async_trait::async_trait]
        impl ServiceManager for MockService {
            async fn install(&self, _config: &DaemonConfig) -> Result<()> {
                Ok(())
            }

            async fn uninstall(&self) -> Result<()> {
                Ok(())
            }

            async fn start(&self) -> Result<()> {
                Ok(())
            }

            async fn stop(&self) -> Result<()> {
                Ok(())
            }

            async fn status(&self) -> Result<DaemonStatus> {
                Ok(DaemonStatus::Unknown)
            }

            async fn service_status(&self) -> Result<ServiceStatus> {
                Ok(ServiceStatus::NotInstalled)
            }
        }

        let service: Box<dyn ServiceManager> = Box::new(MockService);
        let result = service.service_status().await;
        assert!(result.is_ok());
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test --lib test_service_manager_trait_exists`
Expected: FAIL with "cannot find trait `ServiceManager`"

### Step 3: Create ServiceManager trait

```rust
// core/src/daemon/service_manager.rs
use crate::daemon::{DaemonConfig, DaemonError, DaemonStatus, Result, ServiceStatus};
use async_trait::async_trait;

/// Cross-platform service management interface
#[async_trait]
pub trait ServiceManager: Send + Sync {
    /// Install the daemon as a system service
    async fn install(&self, config: &DaemonConfig) -> Result<()>;

    /// Uninstall the daemon service
    async fn uninstall(&self) -> Result<()>;

    /// Start the daemon service
    async fn start(&self) -> Result<()>;

    /// Stop the daemon service
    async fn stop(&self) -> Result<()>;

    /// Get current daemon runtime status
    async fn status(&self) -> Result<DaemonStatus>;

    /// Get service installation status
    async fn service_status(&self) -> Result<ServiceStatus>;
}

/// Create platform-specific service manager
pub fn create_service_manager() -> Result<Box<dyn ServiceManager>> {
    #[cfg(target_os = "macos")]
    {
        use super::platforms::launchd::LaunchdService;
        Ok(Box::new(LaunchdService::new()))
    }

    #[cfg(target_os = "linux")]
    {
        Err(DaemonError::ServiceError("Linux support not yet implemented".to_string()))
    }

    #[cfg(target_os = "windows")]
    {
        Err(DaemonError::ServiceError("Windows support not yet implemented".to_string()))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(DaemonError::ServiceError("Unsupported platform".to_string()))
    }
}
```

### Step 4: Update daemon/mod.rs

```rust
// core/src/daemon/mod.rs
pub mod error;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

### Step 5: Run test to verify it passes

Run: `cargo test --lib test_service_manager_trait_exists`
Expected: PASS

### Step 6: Commit

```bash
git add core/src/daemon/service_manager.rs
git add core/src/daemon/mod.rs
git add core/src/daemon/tests/service_manager_tests.rs
git commit -m "daemon: add ServiceManager trait for cross-platform service management"
```

---

## Task 3: Implement macOS LaunchdService

**Files:**
- Create: `core/src/daemon/platforms/mod.rs`
- Create: `core/src/daemon/platforms/launchd.rs`
- Create: `core/src/daemon/tests/launchd_tests.rs`
- Modify: `core/Cargo.toml`

### Step 1: Add plist dependency

```toml
# core/Cargo.toml (add after line 143)
plist = "1.7"
```

### Step 2: Write the failing test for LaunchdService

```rust
// core/src/daemon/tests/launchd_tests.rs
#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use crate::daemon::*;
    use crate::daemon::platforms::launchd::LaunchdService;

    #[tokio::test]
    async fn test_launchd_service_creation() {
        let service = LaunchdService::new();
        assert!(service.plist_path().to_string_lossy().contains("LaunchAgents"));
    }

    #[tokio::test]
    async fn test_launchd_generate_plist() {
        let service = LaunchdService::new();
        let config = DaemonConfig::default();
        let plist = service.generate_plist(&config).unwrap();
        assert!(plist.contains("com.aether.daemon"));
        assert!(plist.contains("RunAtLoad"));
        assert!(plist.contains("KeepAlive"));
    }
}
```

### Step 3: Run test to verify it fails

Run: `cargo test --lib test_launchd_service_creation`
Expected: FAIL with "cannot find module `platforms`"

### Step 4: Create LaunchdService implementation

```rust
// core/src/daemon/platforms/mod.rs
#[cfg(target_os = "macos")]
pub mod launchd;
```

```rust
// core/src/daemon/platforms/launchd.rs
use crate::daemon::{
    DaemonConfig, DaemonError, DaemonStatus, Result, ServiceManager, ServiceStatus,
};
use async_trait::async_trait;
use plist::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;

const LAUNCHD_LABEL: &str = "com.aether.daemon";

pub struct LaunchdService {
    plist_path: PathBuf,
}

impl LaunchdService {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let plist_path = PathBuf::from(format!(
            "{}/Library/LaunchAgents/{}.plist",
            home, LAUNCHD_LABEL
        ));

        Self { plist_path }
    }

    pub fn plist_path(&self) -> &Path {
        &self.plist_path
    }

    /// Generate launchd plist content
    pub fn generate_plist(&self, config: &DaemonConfig) -> Result<String> {
        let mut dict = HashMap::new();

        // Label
        dict.insert(
            "Label".to_string(),
            Value::String(LAUNCHD_LABEL.to_string()),
        );

        // Program arguments
        let program_args = vec![
            Value::String(config.binary_path.to_string_lossy().to_string()),
            Value::String("daemon".to_string()),
            Value::String("run".to_string()),
        ];
        dict.insert("ProgramArguments".to_string(), Value::Array(program_args));

        // Run at load
        dict.insert("RunAtLoad".to_string(), Value::Boolean(true));

        // Keep alive
        dict.insert("KeepAlive".to_string(), Value::Boolean(true));

        // Standard output/error
        let log_dir = config.log_dir.to_string_lossy().to_string();
        dict.insert(
            "StandardOutPath".to_string(),
            Value::String(format!("{}/daemon.log", log_dir)),
        );
        dict.insert(
            "StandardErrorPath".to_string(),
            Value::String(format!("{}/daemon-error.log", log_dir)),
        );

        // Process type (background daemon)
        dict.insert(
            "ProcessType".to_string(),
            Value::String("Background".to_string()),
        );

        // Nice value (priority)
        dict.insert("Nice".to_string(), Value::Integer(config.nice_value.into()));

        // Resource limits
        let mut soft_limits = HashMap::new();
        soft_limits.insert(
            "MemoryLimit".to_string(),
            Value::Integer(config.soft_mem_limit as i64),
        );
        dict.insert(
            "SoftResourceLimits".to_string(),
            Value::Dictionary(soft_limits),
        );

        let mut hard_limits = HashMap::new();
        hard_limits.insert(
            "MemoryLimit".to_string(),
            Value::Integer(config.hard_mem_limit as i64),
        );
        dict.insert(
            "HardResourceLimits".to_string(),
            Value::Dictionary(hard_limits),
        );

        // Serialize to XML plist
        let plist_value = Value::Dictionary(dict);
        let mut buf = Vec::new();
        plist::to_writer_xml(&mut buf, &plist_value)
            .map_err(|e| DaemonError::Config(format!("Failed to generate plist: {}", e)))?;

        String::from_utf8(buf)
            .map_err(|e| DaemonError::Config(format!("Invalid UTF-8 in plist: {}", e)))
    }

    /// Check if launchd service is loaded
    async fn is_loaded(&self) -> Result<bool> {
        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()
            .await?;

        Ok(output.status.success())
    }
}

#[async_trait]
impl ServiceManager for LaunchdService {
    async fn install(&self, config: &DaemonConfig) -> Result<()> {
        // Ensure log directory exists
        fs::create_dir_all(&config.log_dir).await?;

        // Generate plist content
        let plist_content = self.generate_plist(config)?;

        // Ensure LaunchAgents directory exists
        if let Some(parent) = self.plist_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write plist file
        fs::write(&self.plist_path, plist_content).await?;

        tracing::info!(
            "Installed launchd service at {}",
            self.plist_path.display()
        );

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        // Stop service if running
        if self.is_loaded().await? {
            self.stop().await?;
        }

        // Remove plist file
        if self.plist_path.exists() {
            fs::remove_file(&self.plist_path).await?;
            tracing::info!("Removed launchd plist at {}", self.plist_path.display());
        }

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if !self.plist_path.exists() {
            return Err(DaemonError::ServiceError(
                "Service not installed. Run 'aether daemon install' first.".to_string(),
            ));
        }

        // Load service
        let output = Command::new("launchctl")
            .args(["load", self.plist_path.to_str().unwrap()])
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ServiceError(format!(
                "Failed to start service: {}",
                error
            )));
        }

        tracing::info!("Started launchd service {}", LAUNCHD_LABEL);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if !self.is_loaded().await? {
            return Ok(()); // Already stopped
        }

        // Unload service
        let output = Command::new("launchctl")
            .args(["unload", self.plist_path.to_str().unwrap()])
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ServiceError(format!(
                "Failed to stop service: {}",
                error
            )));
        }

        tracing::info!("Stopped launchd service {}", LAUNCHD_LABEL);
        Ok(())
    }

    async fn status(&self) -> Result<DaemonStatus> {
        if !self.is_loaded().await? {
            return Ok(DaemonStatus::Stopped);
        }

        // Check if process is actually running
        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse PID from output (line like: "PID    Status  Label")
            if stdout.contains("PID") || stdout.contains(LAUNCHD_LABEL) {
                return Ok(DaemonStatus::Running);
            }
        }

        Ok(DaemonStatus::Unknown)
    }

    async fn service_status(&self) -> Result<ServiceStatus> {
        if self.plist_path.exists() {
            Ok(ServiceStatus::Installed)
        } else {
            Ok(ServiceStatus::NotInstalled)
        }
    }
}
```

### Step 5: Run test to verify it passes

Run: `cargo test --lib test_launchd_service_creation`
Expected: PASS

### Step 6: Commit

```bash
git add core/Cargo.toml
git add core/src/daemon/platforms/
git add core/src/daemon/tests/launchd_tests.rs
git commit -m "daemon: implement macOS LaunchdService for service management"
```

---

## Task 4: Implement Resource Governor

**Files:**
- Create: `core/src/daemon/resource_governor.rs`
- Modify: `core/src/daemon/mod.rs`
- Create: `core/src/daemon/tests/resource_governor_tests.rs`
- Modify: `core/Cargo.toml`

### Step 1: Add dependencies

```toml
# core/Cargo.toml (add after line 143)
sysinfo = "0.32"
battery = "0.7"
```

### Step 2: Write the failing test for ResourceGovernor

```rust
// core/src/daemon/tests/resource_governor_tests.rs
#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_resource_governor_creation() {
        let governor = ResourceGovernor::new(
            ResourceLimits {
                cpu_threshold: 20.0,
                mem_threshold: 512 * 1024 * 1024,
                battery_threshold: 20.0,
            }
        );

        assert_eq!(governor.limits().cpu_threshold, 20.0);
    }

    #[tokio::test]
    async fn test_resource_governor_check() {
        let governor = ResourceGovernor::new(ResourceLimits::default());
        let decision = governor.check().await;

        // Should return either Proceed or Throttle
        assert!(matches!(decision, Ok(GovernorDecision::Proceed) | Ok(GovernorDecision::Throttle)));
    }
}
```

### Step 3: Run test to verify it fails

Run: `cargo test --lib test_resource_governor_creation`
Expected: FAIL with "cannot find type `ResourceGovernor`"

### Step 4: Implement ResourceGovernor

```rust
// core/src/daemon/resource_governor.rs
use crate::daemon::{DaemonError, Result};
use sysinfo::{System, RefreshKind, CpuRefreshKind};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Resource limits configuration
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// CPU usage threshold (percentage)
    pub cpu_threshold: f32,

    /// Memory usage threshold (bytes)
    pub mem_threshold: u64,

    /// Battery level threshold (percentage)
    pub battery_threshold: f32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_threshold: 20.0,
            mem_threshold: 512 * 1024 * 1024, // 512MB
            battery_threshold: 20.0,
        }
    }
}

/// Governor decision on whether to proceed with proactive tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorDecision {
    /// System resources are available, proceed with tasks
    Proceed,

    /// System is under stress, throttle proactive tasks
    Throttle,
}

/// Resource Governor monitors system resources and throttles operations
pub struct ResourceGovernor {
    limits: ResourceLimits,
    system: Arc<RwLock<System>>,
}

impl ResourceGovernor {
    /// Create a new ResourceGovernor with specified limits
    pub fn new(limits: ResourceLimits) -> Self {
        let refresh_kind = RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage());

        Self {
            limits,
            system: Arc::new(RwLock::new(System::new_with_specifics(refresh_kind))),
        }
    }

    /// Get current resource limits
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Check system resources and return decision
    pub async fn check(&self) -> Result<GovernorDecision> {
        let mut system = self.system.write().await;

        // Refresh system information
        system.refresh_cpu_usage();
        system.refresh_memory();

        // Check CPU usage
        let cpu_usage = system.global_cpu_usage();
        if cpu_usage > self.limits.cpu_threshold {
            debug!(
                "CPU usage ({:.1}%) exceeds threshold ({:.1}%)",
                cpu_usage, self.limits.cpu_threshold
            );
            return Ok(GovernorDecision::Throttle);
        }

        // Check memory usage (process-specific)
        let pid = sysinfo::get_current_pid()
            .map_err(|e| DaemonError::ResourceGovernor(format!("Failed to get PID: {}", e)))?;

        if let Some(process) = system.process(pid) {
            let mem_usage = process.memory();
            if mem_usage > self.limits.mem_threshold {
                warn!(
                    "Memory usage ({} bytes) exceeds threshold ({} bytes)",
                    mem_usage, self.limits.mem_threshold
                );
                return Ok(GovernorDecision::Throttle);
            }
        }

        // Check battery level (if on battery power)
        if let Ok(manager) = battery::Manager::new() {
            if let Ok(batteries) = manager.batteries() {
                for battery_result in batteries {
                    if let Ok(battery) = battery_result {
                        let state = battery.state();
                        let level = battery.state_of_charge().value * 100.0;

                        // Only throttle if on battery and below threshold
                        if matches!(state, battery::State::Discharging)
                            && level < self.limits.battery_threshold
                        {
                            debug!(
                                "Battery level ({:.1}%) below threshold ({:.1}%)",
                                level, self.limits.battery_threshold
                            );
                            return Ok(GovernorDecision::Throttle);
                        }
                    }
                }
            }
        }

        Ok(GovernorDecision::Proceed)
    }

    /// Check if it's safe to run proactive tasks
    pub async fn is_safe_to_run(&self) -> bool {
        matches!(self.check().await, Ok(GovernorDecision::Proceed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_governor_default_limits() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.cpu_threshold, 20.0);
        assert_eq!(limits.mem_threshold, 512 * 1024 * 1024);
        assert_eq!(limits.battery_threshold, 20.0);
    }
}
```

### Step 5: Update daemon/mod.rs

```rust
// core/src/daemon/mod.rs
pub mod error;
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

### Step 6: Run test to verify it passes

Run: `cargo test --lib test_resource_governor_creation`
Expected: PASS

### Step 7: Commit

```bash
git add core/Cargo.toml
git add core/src/daemon/resource_governor.rs
git add core/src/daemon/mod.rs
git add core/src/daemon/tests/resource_governor_tests.rs
git commit -m "daemon: implement ResourceGovernor for system resource monitoring"
```

---

## Task 5: Implement IPC Server (Unix Domain Socket)

**Files:**
- Create: `core/src/daemon/ipc/mod.rs`
- Create: `core/src/daemon/ipc/server.rs`
- Create: `core/src/daemon/ipc/protocol.rs`
- Create: `core/src/daemon/tests/ipc_tests.rs`
- Modify: `core/src/daemon/mod.rs`

### Step 1: Write the failing test for IPC server

```rust
// core/src/daemon/tests/ipc_tests.rs
#[cfg(test)]
mod tests {
    use crate::daemon::*;
    use crate::daemon::ipc::*;
    use tokio::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_ipc_server_creation() {
        let socket_path = "/tmp/aether-test.sock";
        let _ = std::fs::remove_file(socket_path); // Clean up if exists

        let server = IpcServer::new(socket_path.to_string());
        assert_eq!(server.socket_path(), socket_path);
    }

    #[tokio::test]
    async fn test_json_rpc_request_parsing() {
        let request_json = r#"{"jsonrpc":"2.0","method":"daemon.status","id":1}"#;
        let request: JsonRpcRequest = serde_json::from_str(request_json).unwrap();

        assert_eq!(request.method, "daemon.status");
        assert_eq!(request.id, serde_json::json!(1));
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test --lib test_ipc_server_creation`
Expected: FAIL with "cannot find module `ipc`"

### Step 3: Implement IPC protocol types

```rust
// core/src/daemon/ipc/protocol.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    pub id: Value,
}

/// JSON-RPC 2.0 Response (success)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Value,
    pub id: Value,
}

/// JSON-RPC 2.0 Error Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub error: ErrorObject,
    pub id: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result,
            id,
        }
    }
}

impl JsonRpcError {
    pub fn new(id: Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            error: ErrorObject {
                code,
                message,
                data: None,
            },
            id,
        }
    }
}

// Standard JSON-RPC error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;
```

### Step 4: Implement IPC server

```rust
// core/src/daemon/ipc/server.rs
use crate::daemon::{DaemonError, DaemonStatus, Result};
use crate::daemon::ipc::protocol::*;
use serde_json::Value;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info};

pub struct IpcServer {
    socket_path: String,
}

impl IpcServer {
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Start the IPC server
    pub async fn start(&self) -> Result<()> {
        // Remove existing socket file if it exists
        if Path::new(&self.socket_path).exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        // Bind to Unix Domain Socket
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| DaemonError::IpcError(format!("Failed to bind socket: {}", e)))?;

        info!("IPC server listening on {}", self.socket_path);

        // Accept connections
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream).await {
                            error!("Error handling connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a single client connection
    async fn handle_connection(stream: UnixStream) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;

            if bytes_read == 0 {
                // Connection closed
                break;
            }

            debug!("Received request: {}", line.trim());

            // Parse JSON-RPC request
            let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => Self::handle_request(request).await,
                Err(e) => {
                    let error = JsonRpcError::new(
                        Value::Null,
                        PARSE_ERROR,
                        format!("Parse error: {}", e),
                    );
                    serde_json::to_string(&error).unwrap()
                }
            };

            // Send response
            writer.write_all(response.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }

        Ok(())
    }

    /// Handle a single JSON-RPC request
    async fn handle_request(request: JsonRpcRequest) -> String {
        let result = match request.method.as_str() {
            "daemon.status" => Self::handle_status(request.id).await,
            "daemon.ping" => Self::handle_ping(request.id).await,
            "daemon.shutdown" => Self::handle_shutdown(request.id).await,
            _ => {
                let error = JsonRpcError::new(
                    request.id,
                    METHOD_NOT_FOUND,
                    format!("Method not found: {}", request.method),
                );
                serde_json::to_string(&error).unwrap()
            }
        };

        result
    }

    async fn handle_status(id: Value) -> String {
        let status = DaemonStatus::Running; // Always running if we can respond
        let result = serde_json::json!({
            "status": status,
            "uptime": 0, // TODO: Track actual uptime
        });

        let response = JsonRpcResponse::new(id, result);
        serde_json::to_string(&response).unwrap()
    }

    async fn handle_ping(id: Value) -> String {
        let response = JsonRpcResponse::new(id, serde_json::json!({"pong": true}));
        serde_json::to_string(&response).unwrap()
    }

    async fn handle_shutdown(id: Value) -> String {
        // TODO: Implement graceful shutdown
        let response = JsonRpcResponse::new(id, serde_json::json!({"shutting_down": true}));
        serde_json::to_string(&response).unwrap()
    }
}
```

### Step 5: Create IPC module

```rust
// core/src/daemon/ipc/mod.rs
pub mod protocol;
pub mod server;

pub use protocol::*;
pub use server::IpcServer;
```

### Step 6: Update daemon/mod.rs

```rust
// core/src/daemon/mod.rs
pub mod error;
pub mod ipc;
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use error::{DaemonError, Result};
pub use ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

### Step 7: Run test to verify it passes

Run: `cargo test --lib test_ipc_server_creation`
Expected: PASS

### Step 8: Commit

```bash
git add core/src/daemon/ipc/
git add core/src/daemon/mod.rs
git add core/src/daemon/tests/ipc_tests.rs
git commit -m "daemon: implement IPC server with JSON-RPC 2.0 protocol"
```

---

## Task 6: Create CLI Commands for Daemon Management

**Files:**
- Create: `core/src/daemon/cli.rs`
- Modify: `core/src/daemon/mod.rs`
- Create: `core/src/daemon/tests/cli_tests.rs`

### Step 1: Write the failing test for CLI commands

```rust
// core/src/daemon/tests/cli_tests.rs
#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_daemon_cli_command_parsing() {
        use clap::Parser;

        let args = vec!["aether", "daemon", "install"];
        let cli = DaemonCli::try_parse_from(args);
        assert!(cli.is_ok());

        let cli = cli.unwrap();
        assert!(matches!(cli.command, DaemonCommand::Install));
    }
}
```

### Step 2: Run test to verify it fails

Run: `cargo test --lib test_daemon_cli_command_parsing`
Expected: FAIL with "cannot find type `DaemonCli`"

### Step 3: Implement CLI commands

```rust
// core/src/daemon/cli.rs
use crate::daemon::{create_service_manager, DaemonConfig, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info};

#[derive(Debug, Parser)]
#[command(name = "daemon")]
#[command(about = "Manage Aether daemon service")]
pub struct DaemonCli {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Install daemon as system service
    Install,

    /// Uninstall daemon service
    Uninstall,

    /// Start daemon service
    Start,

    /// Stop daemon service
    Stop,

    /// Check daemon status
    Status,

    /// Run daemon in foreground (for development)
    Run,
}

impl DaemonCli {
    pub async fn execute(&self) -> Result<()> {
        match &self.command {
            DaemonCommand::Install => self.install().await,
            DaemonCommand::Uninstall => self.uninstall().await,
            DaemonCommand::Start => self.start().await,
            DaemonCommand::Stop => self.stop().await,
            DaemonCommand::Status => self.status().await,
            DaemonCommand::Run => self.run().await,
        }
    }

    async fn install(&self) -> Result<()> {
        info!("Installing Aether daemon service...");

        let service = create_service_manager()?;
        let config = DaemonConfig::default();

        service.install(&config).await?;

        info!("✓ Daemon service installed successfully");
        info!("  Run 'aether daemon start' to start the service");

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        info!("Uninstalling Aether daemon service...");

        let service = create_service_manager()?;
        service.uninstall().await?;

        info!("✓ Daemon service uninstalled successfully");

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Aether daemon service...");

        let service = create_service_manager()?;
        service.start().await?;

        info!("✓ Daemon service started successfully");

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Aether daemon service...");

        let service = create_service_manager()?;
        service.stop().await?;

        info!("✓ Daemon service stopped successfully");

        Ok(())
    }

    async fn status(&self) -> Result<()> {
        let service = create_service_manager()?;

        let service_status = service.service_status().await?;
        let daemon_status = service.status().await?;

        info!("Aether Daemon Status:");
        info!("  Service: {:?}", service_status);
        info!("  Daemon:  {:?}", daemon_status);

        Ok(())
    }

    async fn run(&self) -> Result<()> {
        use crate::daemon::ipc::IpcServer;

        info!("Starting Aether daemon in foreground mode...");
        info!("Press Ctrl+C to stop");

        let config = DaemonConfig::default();
        let server = IpcServer::new(config.socket_path);

        // Start IPC server (blocks until Ctrl+C)
        server.start().await?;

        Ok(())
    }
}
```

### Step 4: Update daemon/mod.rs

```rust
// core/src/daemon/mod.rs
pub mod cli;
pub mod error;
pub mod ipc;
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

#[cfg(test)]
mod tests;

pub use cli::{DaemonCli, DaemonCommand};
pub use error::{DaemonError, Result};
pub use ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{ServiceManager, create_service_manager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

### Step 5: Run test to verify it passes

Run: `cargo test --lib test_daemon_cli_command_parsing`
Expected: PASS

### Step 6: Commit

```bash
git add core/src/daemon/cli.rs
git add core/src/daemon/mod.rs
git add core/src/daemon/tests/cli_tests.rs
git commit -m "daemon: add CLI commands for service management"
```

---

## Task 7: Integration Test - Full Daemon Lifecycle

**Files:**
- Create: `core/src/daemon/tests/integration_tests.rs`

### Step 1: Write integration test

```rust
// core/src/daemon/tests/integration_tests.rs
#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    #[ignore] // Run manually: cargo test --lib -- --ignored
    async fn test_daemon_full_lifecycle() {
        // This test requires sudo/admin privileges

        let service = create_service_manager().unwrap();
        let config = DaemonConfig::default();

        // 1. Install
        println!("Installing service...");
        service.install(&config).await.unwrap();

        let status = service.service_status().await.unwrap();
        assert_eq!(status, ServiceStatus::Installed);

        // 2. Start
        println!("Starting service...");
        service.start().await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let status = service.status().await.unwrap();
        assert_eq!(status, DaemonStatus::Running);

        // 3. Stop
        println!("Stopping service...");
        service.stop().await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let status = service.status().await.unwrap();
        assert_eq!(status, DaemonStatus::Stopped);

        // 4. Uninstall
        println!("Uninstalling service...");
        service.uninstall().await.unwrap();

        let status = service.service_status().await.unwrap();
        assert_eq!(status, ServiceStatus::NotInstalled);

        println!("✓ Full lifecycle test passed");
    }
}
```

### Step 2: Run integration test manually

Run: `cargo test --lib test_daemon_full_lifecycle -- --ignored --nocapture`
Expected: Test should pass (requires manual execution due to system-level operations)

### Step 3: Commit

```bash
git add core/src/daemon/tests/integration_tests.rs
git commit -m "daemon: add full lifecycle integration test"
```

---

## Task 8: Documentation and README

**Files:**
- Create: `core/src/daemon/README.md`

### Step 1: Create daemon documentation

```markdown
# Daemon Module

The Daemon module provides system service management for Aether, enabling it to run persistently in the background.

## Architecture

```
┌─────────────────────────────────────────────┐
│           Daemon Module                      │
├─────────────────────────────────────────────┤
│                                             │
│  ┌──────────────┐  ┌──────────────────┐   │
│  │ ServiceManager│  │ ResourceGovernor │   │
│  │  (launchd)   │  │  (CPU/Mem/Bat)   │   │
│  └──────────────┘  └──────────────────┘   │
│                                             │
│  ┌──────────────────────────────────────┐  │
│  │        IPC Server                     │  │
│  │  (Unix Socket + JSON-RPC 2.0)        │  │
│  └──────────────────────────────────────┘  │
│                                             │
└─────────────────────────────────────────────┘
```

## Components

### ServiceManager

Cross-platform trait for system service management:

- **LaunchdService** (macOS): Manages launchd plist and service lifecycle
- **SystemdService** (Linux): TODO
- **WindowsService** (Windows): TODO

### ResourceGovernor

Monitors system resources and throttles operations:

- CPU usage monitoring
- Memory usage tracking
- Battery level detection
- Automatic throttling under high load

### IPC Server

JSON-RPC 2.0 server over Unix Domain Socket:

- Socket path: `~/.aether/daemon.sock`
- Methods:
  - `daemon.status` - Get daemon status
  - `daemon.ping` - Health check
  - `daemon.shutdown` - Graceful shutdown

## Usage

### CLI Commands

```bash
# Install daemon as system service
aether daemon install

# Start daemon
aether daemon start

# Check status
aether daemon status

# Stop daemon
aether daemon stop

# Uninstall daemon
aether daemon uninstall

# Run in foreground (development)
aether daemon run
```

### Programmatic Usage

```rust
use aethecore::daemon::*;

// Create service manager
let service = create_service_manager()?;

// Install and start
let config = DaemonConfig::default();
service.install(&config).await?;
service.start().await?;

// Resource governor
let governor = ResourceGovernor::new(ResourceLimits::default());
if governor.is_safe_to_run().await {
    // Proceed with proactive tasks
}
```

## Configuration

Default configuration:

```rust
DaemonConfig {
    socket_path: "~/.aether/daemon.sock",
    binary_path: "~/.aether/bin/aether-daemon",
    log_dir: "~/.aether/logs",
    nice_value: 10,
    soft_mem_limit: 512 * 1024 * 1024,  // 512MB
    hard_mem_limit: 1024 * 1024 * 1024, // 1GB
}
```

## Testing

```bash
# Unit tests
cargo test --lib daemon::

# Integration test (requires admin privileges)
cargo test --lib test_daemon_full_lifecycle -- --ignored --nocapture
```

## Platform Support

- ✅ macOS (launchd)
- ⏳ Linux (systemd) - Planned
- ⏳ Windows (Service) - Planned
```

### Step 2: Commit documentation

```bash
git add core/src/daemon/README.md
git commit -m "docs: add daemon module documentation"
```

---

## Summary

After completing all tasks, Phase 1 implementation provides:

1. **Daemon Module Foundation** - Core types, errors, and module structure
2. **ServiceManager Trait** - Cross-platform service management interface
3. **LaunchdService** - macOS launchd implementation with full lifecycle
4. **ResourceGovernor** - System resource monitoring and throttling
5. **IPC Server** - Unix Domain Socket with JSON-RPC 2.0 protocol
6. **CLI Commands** - User-friendly daemon management commands
7. **Integration Tests** - Full lifecycle verification
8. **Documentation** - Comprehensive module documentation

### Next Steps

After Phase 1 completion:

1. Proceed to **Phase 2: Perception Layer** - Implement Watchers (Process, File, Time, System)
2. Integrate daemon with existing Gateway infrastructure
3. Add daemon auto-start on system boot verification

### Verification Checklist

- [ ] All unit tests pass: `cargo test --lib daemon::`
- [ ] Integration test passes: `cargo test --lib test_daemon_full_lifecycle -- --ignored`
- [ ] CLI commands work: `aether daemon install/start/stop/status`
- [ ] Daemon runs in background without errors
- [ ] IPC communication works (can send JSON-RPC requests)
- [ ] Resource governor correctly throttles under high load
- [ ] Documentation is complete and accurate

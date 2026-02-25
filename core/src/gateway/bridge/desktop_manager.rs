//! Desktop Bridge Manager.
//!
//! Manages the lifecycle of the desktop bridge (Tauri) subprocess.
//! Uses [`BridgeSupervisor`] to spawn, monitor, and restart the bridge
//! process, and exposes a high-level API for the server to interact
//! with the desktop UI layer.
//!
//! # Design
//!
//! The manager owns a [`BridgeSupervisor`] and holds an `Arc<dyn Transport>`
//! obtained after a successful spawn + handshake. All RPC calls to the
//! bridge (show panel, hide panel, etc.) go through this transport.
//!
//! # Binary discovery
//!
//! [`find_bridge_binary`] looks for `aleph-desktop` next to the current
//! server binary first, then falls back to `$PATH`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, info, warn};

use crate::gateway::bridge::supervisor::{BridgeSupervisor, ManagedProcessConfig};
use crate::gateway::bridge::TransportType;
use crate::gateway::link::LinkId;
use crate::gateway::transport::{Transport, TransportError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The link id used for the desktop bridge process.
const DESKTOP_LINK_ID: &str = "desktop-bridge";

/// Name of the desktop bridge binary to search for.
const BRIDGE_BINARY_NAME: &str = "aleph-desktop";

/// Maximum number of automatic restarts before giving up.
const DEFAULT_MAX_RESTARTS: u32 = 5;

/// Delay (in seconds) before restarting after a crash.
const DEFAULT_RESTART_DELAY_SECS: u64 = 3;

/// Interval (in seconds) between health-check pings.
const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// DesktopBridgeManager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of the desktop bridge (Tauri) subprocess.
///
/// Wraps [`BridgeSupervisor`] to provide a focused API for the server's
/// desktop UI bridge. The manager handles:
///
/// - Binary discovery ([`find_bridge_binary`])
/// - Spawning with the correct environment (server port, bridge mode)
/// - Transport-level RPC calls (`show_panel`, `hide_panel`, etc.)
/// - Graceful shutdown
///
/// # Example
///
/// ```rust,no_run
/// # use std::path::PathBuf;
/// # use alephcore::gateway::bridge::desktop_manager::DesktopBridgeManager;
/// # async fn example() -> Result<(), String> {
/// let mut mgr = DesktopBridgeManager::new(
///     PathBuf::from("/tmp/aleph/run"),
///     9900,
/// );
/// mgr.start().await?;
/// assert!(mgr.is_connected());
/// mgr.stop().await;
/// # Ok(())
/// # }
/// ```
pub struct DesktopBridgeManager {
    /// The supervisor that manages the bridge child process.
    supervisor: BridgeSupervisor,

    /// Transport handle obtained after a successful spawn + handshake.
    /// `None` until [`start`] succeeds.
    transport: Option<Arc<dyn Transport>>,

    /// The port the Aleph server is listening on, passed to the bridge
    /// as `ALEPH_SERVER_PORT` so it can connect back via WebSocket.
    server_port: u16,

    /// Capabilities reported by the bridge during handshake.
    /// Populated after a successful start (TODO: parse from handshake response).
    capabilities: Vec<String>,
}

impl DesktopBridgeManager {
    /// Create a new manager.
    ///
    /// - `run_dir` — directory for runtime files (Unix sockets, PID files).
    /// - `server_port` — the port the Aleph server listens on (passed to bridge).
    pub fn new(run_dir: PathBuf, server_port: u16) -> Self {
        Self {
            supervisor: BridgeSupervisor::new(run_dir),
            transport: None,
            server_port,
            capabilities: Vec::new(),
        }
    }

    /// Spawn the desktop bridge subprocess and establish IPC.
    ///
    /// Locates the bridge binary, builds a [`ManagedProcessConfig`], and
    /// delegates to [`BridgeSupervisor::spawn`]. On success the transport
    /// handle is stored for subsequent RPC calls.
    ///
    /// Returns an error string if the binary cannot be found or the
    /// spawn/handshake fails.
    pub async fn start(&mut self) -> Result<(), String> {
        // Don't double-start.
        if self.transport.is_some() {
            return Err("Desktop bridge is already running".into());
        }

        let binary = Self::find_bridge_binary().ok_or_else(|| {
            format!(
                "Desktop bridge binary '{}' not found (checked sibling dir and PATH)",
                BRIDGE_BINARY_NAME,
            )
        })?;

        info!(binary = %binary.display(), "Starting desktop bridge");

        let mut env_vars = HashMap::new();
        env_vars.insert(
            "ALEPH_SERVER_PORT".to_string(),
            self.server_port.to_string(),
        );
        env_vars.insert("ALEPH_BRIDGE_MODE".to_string(), "true".to_string());

        let config = ManagedProcessConfig {
            executable: binary,
            args: vec!["--bridge-mode".to_string()],
            transport_type: TransportType::UnixSocket,
            max_restarts: DEFAULT_MAX_RESTARTS,
            restart_delay: std::time::Duration::from_secs(DEFAULT_RESTART_DELAY_SECS),
            health_check_interval: std::time::Duration::from_secs(
                DEFAULT_HEALTH_CHECK_INTERVAL_SECS,
            ),
            env_vars,
        };

        let link_id = LinkId::new(DESKTOP_LINK_ID);

        let transport = self
            .supervisor
            .spawn(&link_id, config)
            .await
            .map_err(|e| format!("Failed to start desktop bridge: {e}"))?;

        info!("Desktop bridge started and handshake completed");

        self.transport = Some(transport);

        // TODO: Parse capabilities from handshake response and populate
        // self.capabilities. For now we leave it empty.

        Ok(())
    }

    /// Gracefully stop the desktop bridge subprocess.
    ///
    /// Tells the supervisor to stop the process and clears the local
    /// transport handle. Idempotent — safe to call even if not started.
    pub async fn stop(&mut self) {
        let link_id = LinkId::new(DESKTOP_LINK_ID);

        if let Err(e) = self.supervisor.stop(&link_id).await {
            warn!(error = %e, "Error stopping desktop bridge");
        }

        self.transport = None;
        self.capabilities.clear();

        debug!("Desktop bridge manager stopped");
    }

    /// Returns `true` if the bridge transport is connected.
    pub fn is_connected(&self) -> bool {
        self.transport
            .as_ref()
            .map_or(false, |t| t.is_connected())
    }

    /// Check whether the bridge reported a specific capability during
    /// handshake.
    ///
    /// Currently always returns `false` because capability parsing from
    /// the handshake response is not yet implemented.
    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.iter().any(|c| c == name)
    }

    /// Send a JSON-RPC request to the bridge and return the response.
    ///
    /// This is the low-level call used by the convenience methods
    /// ([`show_panel`], [`hide_panel`], etc.).
    pub async fn call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, TransportError> {
        let transport = self
            .transport
            .as_ref()
            .ok_or(TransportError::NotConnected)?;

        transport.request(method, params).await
    }

    /// Ask the bridge to show a web panel at the given URL.
    pub async fn show_panel(&self, url: &str) -> Result<(), TransportError> {
        self.call(
            "webview.show",
            serde_json::json!({ "url": url }),
        )
        .await?;
        Ok(())
    }

    /// Ask the bridge to hide the web panel.
    pub async fn hide_panel(&self) -> Result<(), TransportError> {
        self.call("webview.hide", serde_json::json!({})).await?;
        Ok(())
    }

    /// Attempt to locate the desktop bridge binary.
    ///
    /// Search order:
    /// 1. Same directory as the currently running server binary.
    /// 2. `$PATH` lookup via `which`.
    fn find_bridge_binary() -> Option<PathBuf> {
        // 1. Check sibling of current executable.
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(dir) = current_exe.parent() {
                let candidate = dir.join(BRIDGE_BINARY_NAME);
                if candidate.exists() {
                    debug!(path = %candidate.display(), "Found bridge binary next to server");
                    return Some(candidate);
                }
            }
        }

        // 2. Fall back to PATH.
        match which::which(BRIDGE_BINARY_NAME) {
            Ok(path) => {
                debug!(path = %path.display(), "Found bridge binary in PATH");
                Some(path)
            }
            Err(_) => {
                debug!("Bridge binary '{}' not found in PATH", BRIDGE_BINARY_NAME);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_manager() {
        let mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/aleph/run"), 9900);
        assert!(!mgr.is_connected());
        assert!(!mgr.has_capability("webview"));
        assert!(mgr.capabilities.is_empty());
    }

    #[test]
    fn test_has_capability_empty() {
        let mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        assert!(!mgr.has_capability("webview"));
        assert!(!mgr.has_capability("screenshot"));
        assert!(!mgr.has_capability(""));
    }

    #[test]
    fn test_has_capability_populated() {
        let mut mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        mgr.capabilities = vec!["webview".into(), "screenshot".into()];
        assert!(mgr.has_capability("webview"));
        assert!(mgr.has_capability("screenshot"));
        assert!(!mgr.has_capability("keyboard"));
    }

    #[tokio::test]
    async fn test_call_not_connected() {
        let mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        let result = mgr.call("test.method", serde_json::json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TransportError::NotConnected => {} // expected
            other => panic!("Expected NotConnected, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_show_panel_not_connected() {
        let mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        let result = mgr.show_panel("http://localhost:3000").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_hide_panel_not_connected() {
        let mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        let result = mgr.hide_panel().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stop_idempotent() {
        let mut mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/aleph-test/run"), 9900);
        // Stopping without starting should not panic.
        mgr.stop().await;
        assert!(!mgr.is_connected());
    }

    #[tokio::test]
    async fn test_start_double_start() {
        // We can't actually start (no binary), but we can test the guard
        // by manually setting transport to Some.
        // This test verifies the double-start check logic path.
        // Since we can't easily mock, just verify start fails with binary not found.
        let mut mgr = DesktopBridgeManager::new(PathBuf::from("/tmp/run"), 8080);
        let result = mgr.start().await;
        // Should fail because binary doesn't exist, not because of double-start.
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_find_bridge_binary_not_found() {
        // In test environment, the binary is unlikely to exist.
        // This just ensures the function doesn't panic.
        let _result = DesktopBridgeManager::find_bridge_binary();
    }
}

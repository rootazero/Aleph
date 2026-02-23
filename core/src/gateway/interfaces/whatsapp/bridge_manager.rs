//! Bridge Manager — Child Process Lifecycle
//!
//! Manages the Go `whatsapp-bridge` binary as a child process.
//! Handles spawning, monitoring, restart with backoff, and graceful shutdown.
//!
//! # Lifecycle
//!
//! ```text
//!   new(config) ──→ start() ──→ [running] ──→ stop()
//!                                  │
//!                                  ├──→ is_running()
//!                                  ├──→ restart()  (up to max_restarts)
//!                                  └──→ reset_restart_count()
//! ```
//!
//! # Drop Behavior
//!
//! When a `BridgeManager` is dropped, it makes a best-effort attempt to
//! kill the child process and clean up the Unix socket file.

use std::fmt;
use std::path::PathBuf;
use tokio::process::{Child, Command};

// ─── BridgeError ────────────────────────────────────────────────────────────

/// Errors that can occur during bridge process management.
#[derive(Debug)]
pub enum BridgeError {
    /// The whatsapp-bridge binary was not found at the configured path.
    BinaryNotFound(String),
    /// Failed to spawn the child process.
    SpawnFailed(String),
    /// A general I/O error occurred.
    IoError(String),
    /// The bridge has been restarted too many times.
    MaxRestartsExceeded(u32),
    /// The bridge process exited unexpectedly.
    UnexpectedExit(String),
    /// An error related to the Unix domain socket.
    SocketError(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::BinaryNotFound(msg) => write!(f, "Binary not found: {}", msg),
            BridgeError::SpawnFailed(msg) => write!(f, "Spawn failed: {}", msg),
            BridgeError::IoError(msg) => write!(f, "I/O error: {}", msg),
            BridgeError::MaxRestartsExceeded(n) => {
                write!(f, "Max restarts exceeded ({})", n)
            }
            BridgeError::UnexpectedExit(msg) => write!(f, "Unexpected exit: {}", msg),
            BridgeError::SocketError(msg) => write!(f, "Socket error: {}", msg),
        }
    }
}

impl std::error::Error for BridgeError {}

// ─── BridgeManagerConfig ────────────────────────────────────────────────────

/// Configuration for the bridge child process manager.
#[derive(Debug, Clone)]
pub struct BridgeManagerConfig {
    /// Path to the whatsapp-bridge binary.
    pub binary_path: PathBuf,
    /// Unix socket path for IPC with the bridge.
    pub socket_path: PathBuf,
    /// Directory where the bridge stores session data.
    pub data_dir: PathBuf,
    /// Maximum number of automatic restarts before giving up.
    pub max_restarts: u32,
    /// Delay in seconds between a crash and the next restart attempt.
    pub restart_delay_secs: u64,
}

impl Default for BridgeManagerConfig {
    fn default() -> Self {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("whatsapp");

        Self {
            binary_path: PathBuf::from("whatsapp-bridge"),
            socket_path: base_dir.join("bridge.sock"),
            data_dir: base_dir.join("data"),
            max_restarts: 5,
            restart_delay_secs: 3,
        }
    }
}

// ─── BridgeManager ──────────────────────────────────────────────────────────

/// Manages the lifecycle of the Go whatsapp-bridge child process.
///
/// The manager handles:
/// - Spawning the bridge binary with correct arguments
/// - Monitoring whether the process is still alive
/// - Restarting after crashes (with a configurable limit)
/// - Graceful shutdown and socket cleanup
pub struct BridgeManager {
    /// Configuration for the bridge process.
    config: BridgeManagerConfig,
    /// Handle to the running child process, if any.
    child: Option<Child>,
    /// Number of restarts performed since the last reset.
    restart_count: u32,
}

impl BridgeManager {
    /// Create a new `BridgeManager` with the given configuration.
    pub fn new(config: BridgeManagerConfig) -> Self {
        Self {
            config,
            child: None,
            restart_count: 0,
        }
    }

    /// Start the bridge process.
    ///
    /// This will:
    /// 1. Create the data directory if it doesn't exist
    /// 2. Remove any stale Unix socket file
    /// 3. Spawn the bridge binary
    pub async fn start(&mut self) -> Result<(), BridgeError> {
        // 1. Ensure data directory exists
        tokio::fs::create_dir_all(&self.config.data_dir)
            .await
            .map_err(|e| {
                BridgeError::IoError(format!(
                    "Failed to create data dir {:?}: {}",
                    self.config.data_dir, e
                ))
            })?;

        // 2. Clean up stale socket file
        if self.config.socket_path.exists() {
            tokio::fs::remove_file(&self.config.socket_path)
                .await
                .map_err(|e| {
                    BridgeError::SocketError(format!(
                        "Failed to remove stale socket {:?}: {}",
                        self.config.socket_path, e
                    ))
                })?;
        }

        // 3. Spawn the process
        self.spawn_process().await
    }

    /// Spawn the bridge binary as a child process.
    ///
    /// The binary is invoked with `--socket <path>` and `--data-dir <path>`.
    /// The child is configured with `kill_on_drop(true)` so it is killed
    /// if the `BridgeManager` is dropped without an explicit `stop()`.
    async fn spawn_process(&mut self) -> Result<(), BridgeError> {
        // Check that the binary exists on PATH or at the given path
        let binary = &self.config.binary_path;
        if !binary.is_absolute() {
            // For relative / bare names, check PATH
            which::which(binary).map_err(|_| {
                BridgeError::BinaryNotFound(format!(
                    "Binary {:?} not found in PATH",
                    binary
                ))
            })?;
        } else if !binary.exists() {
            return Err(BridgeError::BinaryNotFound(format!(
                "Binary not found at {:?}",
                binary
            )));
        }

        let child = Command::new(&self.config.binary_path)
            .arg("--socket")
            .arg(&self.config.socket_path)
            .arg("--data-dir")
            .arg(&self.config.data_dir)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                BridgeError::SpawnFailed(format!(
                    "Failed to spawn {:?}: {}",
                    self.config.binary_path, e
                ))
            })?;

        tracing::info!(
            pid = child.id().unwrap_or(0),
            binary = ?self.config.binary_path,
            socket = ?self.config.socket_path,
            "WhatsApp bridge process spawned"
        );

        self.child = Some(child);
        Ok(())
    }

    /// Stop the bridge process and clean up the socket file.
    ///
    /// If no child is running, this is a no-op and returns `Ok(())`.
    pub async fn stop(&mut self) -> Result<(), BridgeError> {
        if let Some(ref mut child) = self.child {
            child.kill().await.map_err(|e| {
                BridgeError::IoError(format!("Failed to kill bridge process: {}", e))
            })?;
            // Wait for the process to fully exit
            let _ = child.wait().await;
            tracing::info!("WhatsApp bridge process stopped");
        }
        self.child = None;

        // Clean up socket file
        if self.config.socket_path.exists() {
            let _ = tokio::fs::remove_file(&self.config.socket_path).await;
        }

        Ok(())
    }

    /// Check whether the bridge process is currently running.
    ///
    /// Returns `false` if no child has been spawned, or if the child
    /// has already exited.
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => {
                // try_wait returns Ok(Some(status)) if exited, Ok(None) if still running
                match child.try_wait() {
                    Ok(Some(_)) => {
                        // Process has exited
                        false
                    }
                    Ok(None) => {
                        // Still running
                        true
                    }
                    Err(_) => false,
                }
            }
            None => false,
        }
    }

    /// Restart the bridge process.
    ///
    /// Increments the restart counter and returns `MaxRestartsExceeded`
    /// if the limit has been reached. Otherwise, stops the current
    /// process, waits `restart_delay_secs`, and spawns a new one.
    pub async fn restart(&mut self) -> Result<(), BridgeError> {
        if self.restart_count >= self.config.max_restarts {
            return Err(BridgeError::MaxRestartsExceeded(self.config.max_restarts));
        }

        self.restart_count += 1;
        tracing::info!(
            restart_count = self.restart_count,
            max_restarts = self.config.max_restarts,
            "Restarting WhatsApp bridge"
        );

        // Stop the current process
        self.stop().await?;

        // Wait before restarting
        tokio::time::sleep(tokio::time::Duration::from_secs(
            self.config.restart_delay_secs,
        ))
        .await;

        // Spawn a new process
        self.spawn_process().await
    }

    /// Get a reference to the configured socket path.
    pub fn socket_path(&self) -> &PathBuf {
        &self.config.socket_path
    }

    /// Reset the restart counter.
    ///
    /// Call this after a successful connection to allow future restarts.
    pub fn reset_restart_count(&mut self) {
        self.restart_count = 0;
    }

    /// Get the current restart count.
    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }
}

impl Drop for BridgeManager {
    fn drop(&mut self) {
        // Best-effort kill: tokio::process::Child with kill_on_drop(true)
        // handles this automatically, but we also clean up the child handle.
        if let Some(ref mut child) = self.child {
            // start_kill is non-async and sends SIGKILL
            let _ = child.start_kill();
        }

        // Best-effort socket cleanup (synchronous)
        if self.config.socket_path.exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> BridgeManagerConfig {
        BridgeManagerConfig {
            binary_path: PathBuf::from("whatsapp-bridge"),
            socket_path: PathBuf::from("/tmp/test-bridge.sock"),
            data_dir: PathBuf::from("/tmp/test-bridge-data"),
            max_restarts: 3,
            restart_delay_secs: 0, // No delay in tests
        }
    }

    // ── Default config values ───────────────────────────────────────

    #[test]
    fn test_default_config_values() {
        let config = BridgeManagerConfig::default();

        assert_eq!(config.binary_path, PathBuf::from("whatsapp-bridge"));
        assert_eq!(config.max_restarts, 5);
        assert_eq!(config.restart_delay_secs, 3);

        // Socket and data paths should be under ~/.aleph/whatsapp/
        let socket_str = config.socket_path.to_string_lossy();
        assert!(
            socket_str.contains(".aleph/whatsapp"),
            "socket_path should contain .aleph/whatsapp, got: {}",
            socket_str
        );
        assert!(
            socket_str.ends_with("bridge.sock"),
            "socket_path should end with bridge.sock, got: {}",
            socket_str
        );

        let data_str = config.data_dir.to_string_lossy();
        assert!(
            data_str.contains(".aleph/whatsapp"),
            "data_dir should contain .aleph/whatsapp, got: {}",
            data_str
        );
        assert!(
            data_str.ends_with("data"),
            "data_dir should end with data, got: {}",
            data_str
        );
    }

    // ── Creation ────────────────────────────────────────────────────

    #[test]
    fn test_creation_with_correct_defaults() {
        let config = test_config();
        let manager = BridgeManager::new(config.clone());

        assert!(manager.child.is_none());
        assert_eq!(manager.restart_count, 0);
        assert_eq!(manager.config.max_restarts, 3);
        assert_eq!(
            manager.config.binary_path,
            PathBuf::from("whatsapp-bridge")
        );
    }

    // ── start() with missing binary ─────────────────────────────────

    #[tokio::test]
    async fn test_start_with_missing_binary_returns_binary_not_found() {
        let config = BridgeManagerConfig {
            binary_path: PathBuf::from("nonexistent-whatsapp-bridge-binary-xyz"),
            socket_path: PathBuf::from("/tmp/test-bridge-missing.sock"),
            data_dir: std::env::temp_dir().join("test-bridge-missing-data"),
            max_restarts: 3,
            restart_delay_secs: 0,
        };
        let mut manager = BridgeManager::new(config);

        let result = manager.start().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::BinaryNotFound(msg) => {
                assert!(
                    msg.contains("nonexistent-whatsapp-bridge-binary-xyz"),
                    "Error message should mention the binary name, got: {}",
                    msg
                );
            }
            other => panic!("Expected BinaryNotFound, got: {:?}", other),
        }

        // Cleanup
        let _ = tokio::fs::remove_dir_all(
            std::env::temp_dir().join("test-bridge-missing-data"),
        )
        .await;
    }

    // ── start() with absolute path missing binary ───────────────────

    #[tokio::test]
    async fn test_start_with_absolute_missing_binary() {
        let config = BridgeManagerConfig {
            binary_path: PathBuf::from("/usr/local/bin/nonexistent-wa-bridge"),
            socket_path: PathBuf::from("/tmp/test-bridge-abs.sock"),
            data_dir: std::env::temp_dir().join("test-bridge-abs-data"),
            max_restarts: 3,
            restart_delay_secs: 0,
        };
        let mut manager = BridgeManager::new(config);

        let result = manager.start().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::BinaryNotFound(_) => {}
            other => panic!("Expected BinaryNotFound, got: {:?}", other),
        }

        // Cleanup
        let _ =
            tokio::fs::remove_dir_all(std::env::temp_dir().join("test-bridge-abs-data")).await;
    }

    // ── stop() when not running ─────────────────────────────────────

    #[tokio::test]
    async fn test_stop_when_not_running_is_ok() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);

        // stop() on a manager with no child should succeed
        let result = manager.stop().await;
        assert!(result.is_ok());
    }

    // ── restart limit enforcement ───────────────────────────────────

    #[tokio::test]
    async fn test_restart_limit_enforcement() {
        let config = BridgeManagerConfig {
            binary_path: PathBuf::from("nonexistent-wa-bridge-restart-test"),
            socket_path: PathBuf::from("/tmp/test-bridge-restart.sock"),
            data_dir: std::env::temp_dir().join("test-bridge-restart-data"),
            max_restarts: 2,
            restart_delay_secs: 0,
        };
        let mut manager = BridgeManager::new(config);

        // Manually set restart count to max
        manager.restart_count = 2;

        let result = manager.restart().await;
        assert!(result.is_err());

        match result.unwrap_err() {
            BridgeError::MaxRestartsExceeded(n) => {
                assert_eq!(n, 2);
            }
            other => panic!("Expected MaxRestartsExceeded, got: {:?}", other),
        }

        // Cleanup
        let _ = tokio::fs::remove_dir_all(
            std::env::temp_dir().join("test-bridge-restart-data"),
        )
        .await;
    }

    // ── reset_restart_count ─────────────────────────────────────────

    #[test]
    fn test_reset_restart_count_works() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);

        manager.restart_count = 4;
        assert_eq!(manager.restart_count(), 4);

        manager.reset_restart_count();
        assert_eq!(manager.restart_count(), 0);
    }

    // ── is_running when no child ────────────────────────────────────

    #[test]
    fn test_is_running_when_no_child() {
        let config = test_config();
        let mut manager = BridgeManager::new(config);

        assert!(!manager.is_running());
    }

    // ── socket_path accessor ────────────────────────────────────────

    #[test]
    fn test_socket_path_accessor() {
        let config = test_config();
        let manager = BridgeManager::new(config);

        assert_eq!(
            manager.socket_path(),
            &PathBuf::from("/tmp/test-bridge.sock")
        );
    }

    // ── restart_count accessor ──────────────────────────────────────

    #[test]
    fn test_restart_count_accessor() {
        let config = test_config();
        let manager = BridgeManager::new(config);

        assert_eq!(manager.restart_count(), 0);
    }

    // ── BridgeError Display ─────────────────────────────────────────

    #[test]
    fn test_bridge_error_display() {
        let errors = vec![
            (
                BridgeError::BinaryNotFound("not found".to_string()),
                "Binary not found: not found",
            ),
            (
                BridgeError::SpawnFailed("failed".to_string()),
                "Spawn failed: failed",
            ),
            (
                BridgeError::IoError("io error".to_string()),
                "I/O error: io error",
            ),
            (
                BridgeError::MaxRestartsExceeded(5),
                "Max restarts exceeded (5)",
            ),
            (
                BridgeError::UnexpectedExit("crashed".to_string()),
                "Unexpected exit: crashed",
            ),
            (
                BridgeError::SocketError("socket problem".to_string()),
                "Socket error: socket problem",
            ),
        ];

        for (error, expected) in errors {
            assert_eq!(error.to_string(), expected);
        }
    }

    // ── restart increments count ────────────────────────────────────

    #[tokio::test]
    async fn test_restart_increments_count_before_failure() {
        let config = BridgeManagerConfig {
            binary_path: PathBuf::from("nonexistent-wa-bridge-count-test"),
            socket_path: PathBuf::from("/tmp/test-bridge-count.sock"),
            data_dir: std::env::temp_dir().join("test-bridge-count-data"),
            max_restarts: 5,
            restart_delay_secs: 0,
        };
        let mut manager = BridgeManager::new(config);

        assert_eq!(manager.restart_count(), 0);

        // restart will increment count, then fail at spawn_process (binary not found)
        let result = manager.restart().await;
        assert!(result.is_err()); // Binary not found during spawn
        assert_eq!(manager.restart_count(), 1);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(
            std::env::temp_dir().join("test-bridge-count-data"),
        )
        .await;
    }
}

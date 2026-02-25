//! Bridge process lifecycle manager.
//!
//! [`BridgeSupervisor`] manages external bridge processes — spawning them,
//! monitoring their health via periodic heartbeat pings, and automatically
//! restarting them on failure. Each managed process is identified by its
//! [`LinkId`] and communicates via an [`Arc<dyn Transport>`].
//!
//! # Example
//!
//! ```rust,no_run
//! use std::path::PathBuf;
//! use alephcore::gateway::bridge::supervisor::{BridgeSupervisor, ManagedProcessConfig, SpawnResult};
//! use alephcore::gateway::link::LinkId;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let supervisor = BridgeSupervisor::new(PathBuf::from("/tmp/aleph/run"));
//! let config = ManagedProcessConfig {
//!     executable: PathBuf::from("/usr/local/bin/signal-bridge"),
//!     args: vec!["--verbose".into()],
//!     transport_type: alephcore::gateway::bridge::TransportType::UnixSocket,
//!     max_restarts: 5,
//!     restart_delay: std::time::Duration::from_secs(3),
//!     health_check_interval: std::time::Duration::from_secs(30),
//!     env_vars: std::collections::HashMap::new(),
//! };
//! let result = supervisor.spawn(&LinkId::new("my-signal"), config).await?;
//! let _transport = result.transport;
//! let _handshake = result.handshake_response;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::gateway::bridge::types::{BridgeRuntime, TransportType};
use crate::gateway::link::LinkId;
use crate::gateway::transport::unix_socket::UnixSocketTransport;
use crate::gateway::transport::{StdioTransport, Transport};

// ---------------------------------------------------------------------------
// SpawnResult
// ---------------------------------------------------------------------------

/// Successful result of spawning a bridge process.
///
/// Contains the IPC transport handle and the raw JSON response from the
/// `aleph.handshake` RPC, which typically includes capability declarations
/// and protocol metadata.
pub struct SpawnResult {
    /// The IPC transport handle for communicating with the bridge.
    pub transport: Arc<dyn Transport>,
    /// The JSON response from the `aleph.handshake` RPC.
    pub handshake_response: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ManagedProcessConfig
// ---------------------------------------------------------------------------

/// Configuration for a managed bridge child process.
///
/// Constructed from a [`BridgeRuntime::Process`] variant via
/// [`ManagedProcessConfig::from_runtime`], or manually for testing.
#[derive(Debug, Clone)]
pub struct ManagedProcessConfig {
    /// Path to the bridge executable (absolute or relative).
    pub executable: PathBuf,
    /// Command-line arguments passed to the executable.
    pub args: Vec<String>,
    /// IPC transport type between Aleph and the bridge process.
    pub transport_type: TransportType,
    /// Maximum number of automatic restarts before giving up.
    pub max_restarts: u32,
    /// Delay before restarting after a crash.
    pub restart_delay: Duration,
    /// Interval between health-check pings.
    pub health_check_interval: Duration,
    /// Additional environment variables to set on the child process.
    pub env_vars: HashMap<String, String>,
}

impl ManagedProcessConfig {
    /// Create a [`ManagedProcessConfig`] from a [`BridgeRuntime`].
    ///
    /// Returns `None` for [`BridgeRuntime::Builtin`] since builtin bridges
    /// do not have an external process to manage.
    pub fn from_runtime(runtime: &BridgeRuntime) -> Option<Self> {
        match runtime {
            BridgeRuntime::Builtin => None,
            BridgeRuntime::Process {
                executable,
                args,
                transport,
                health_check_interval_secs,
                max_restarts,
                restart_delay_secs,
            } => Some(Self {
                executable: PathBuf::from(executable.clone()),
                args: args.clone(),
                transport_type: transport.clone(),
                max_restarts: *max_restarts,
                restart_delay: Duration::from_secs(*restart_delay_secs),
                health_check_interval: Duration::from_secs(*health_check_interval_secs),
                env_vars: HashMap::new(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// ProcessStatus
// ---------------------------------------------------------------------------

/// Current lifecycle status of a managed bridge process.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    /// The process is being started for the first time.
    Starting,
    /// The process is running and responding to health checks.
    Running,
    /// The process failed one or more health checks.
    Unhealthy,
    /// The process crashed and is being restarted.
    Restarting,
    /// The process was intentionally stopped.
    Stopped,
    /// The process exceeded its restart limit or hit a fatal error.
    Failed(String),
}

// ---------------------------------------------------------------------------
// ManagedProcess (private)
// ---------------------------------------------------------------------------

/// Internal bookkeeping for a running bridge process.
struct ManagedProcess {
    /// The link this process belongs to.
    #[allow(dead_code)]
    link_id: LinkId,
    /// The child process handle.
    child: Child,
    /// The transport used to communicate with this process.
    transport: Arc<dyn Transport>,
    /// Configuration used to spawn (and potentially restart) this process.
    #[allow(dead_code)]
    config: ManagedProcessConfig,
    /// How many times this process has been restarted.
    #[allow(dead_code)]
    restart_count: u32,
    /// Timestamp of the most recent successful health check.
    last_heartbeat: Instant,
    /// Current status.
    status: ProcessStatus,
}

// ---------------------------------------------------------------------------
// BridgeSupervisorError
// ---------------------------------------------------------------------------

/// Errors that can occur during bridge process lifecycle management.
#[derive(Debug, thiserror::Error)]
pub enum BridgeSupervisorError {
    /// The specified bridge executable was not found on disk or in $PATH.
    #[error("Binary not found: {0}")]
    BinaryNotFound(String),

    /// Failed to spawn the child process.
    #[error("Spawn failed: {0}")]
    SpawnFailed(String),

    /// Failed to establish IPC connection to the bridge process.
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// The initial handshake with the bridge process failed.
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),

    /// An I/O error occurred during process management.
    #[error("IO error: {0}")]
    IoError(String),

    /// The process exceeded its configured maximum restart count.
    #[error("Max restarts exceeded: {0}")]
    MaxRestartsExceeded(u32),
}

// ---------------------------------------------------------------------------
// BridgeSupervisor
// ---------------------------------------------------------------------------

/// Manages the lifecycle of external bridge child processes.
///
/// The supervisor handles spawning, health monitoring, and stopping bridge
/// processes. Each process is identified by its [`LinkId`] and communicates
/// with Aleph through a [`Transport`].
///
/// # Health monitoring
///
/// After a process is spawned, a background tokio task periodically sends
/// `system.ping` requests via the transport. If a ping fails, the process
/// is marked [`ProcessStatus::Unhealthy`]. Consecutive failures may trigger
/// a restart (future enhancement).
///
/// # Thread safety
///
/// All internal state is behind an `Arc<RwLock<...>>`, so the supervisor can
/// be shared across tasks and the health monitor can update state without
/// holding a write lock across await points.
pub struct BridgeSupervisor {
    /// Map of active managed processes keyed by link id.
    processes: Arc<RwLock<HashMap<LinkId, ManagedProcess>>>,
    /// Directory for runtime files (e.g. Unix sockets).
    run_dir: PathBuf,
    /// Shutdown sender: broadcast `true` to stop all health monitor tasks.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Shutdown receiver: cloned into each health monitor task.
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
}

impl BridgeSupervisor {
    /// Create a new supervisor that stores runtime files in `run_dir`.
    pub fn new(run_dir: PathBuf) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
            run_dir,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// List all managed processes and their current status (synchronous).
    ///
    /// # Limitations
    ///
    /// This method uses `try_read()` on the tokio `RwLock`. If the lock is
    /// currently held by a writer, this returns an empty `Vec` rather than
    /// blocking. For reliable results, prefer [`list_processes_async`].
    pub fn list_processes(&self) -> Vec<(LinkId, ProcessStatus)> {
        match self.processes.try_read() {
            Ok(guard) => guard
                .iter()
                .map(|(id, proc)| (id.clone(), proc.status.clone()))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// List all managed processes and their current status (async).
    ///
    /// Acquires a read lock on the process map. Prefer this over
    /// [`list_processes`] when calling from an async context.
    pub async fn list_processes_async(&self) -> Vec<(LinkId, ProcessStatus)> {
        let guard = self.processes.read().await;
        guard
            .iter()
            .map(|(id, proc)| (id.clone(), proc.status.clone()))
            .collect()
    }

    /// Spawn a new bridge process and establish an IPC connection.
    ///
    /// # Steps
    ///
    /// 1. Create `run_dir` if it does not exist.
    /// 2. Clean up stale socket files (for Unix socket transport).
    /// 3. Validate that the executable exists.
    /// 4. Spawn the child process with appropriate env vars.
    /// 5. Create the transport and connect.
    /// 6. Perform the `aleph.handshake` RPC call.
    /// 7. Store the managed process and start health monitoring.
    ///
    /// Returns a [`SpawnResult`] containing the transport handle and the
    /// raw handshake response (which typically includes capability info).
    pub async fn spawn(
        &self,
        link_id: &LinkId,
        config: ManagedProcessConfig,
    ) -> Result<SpawnResult, BridgeSupervisorError> {
        // 1. Ensure run_dir exists.
        tokio::fs::create_dir_all(&self.run_dir)
            .await
            .map_err(|e| BridgeSupervisorError::IoError(format!("create run_dir: {e}")))?;

        // 2. Compute socket path and clean up stale socket.
        let socket_path = self.run_dir.join(format!("{}.sock", link_id.as_str()));
        if config.transport_type == TransportType::UnixSocket && socket_path.exists() {
            let _ = tokio::fs::remove_file(&socket_path).await;
            debug!(path = %socket_path.display(), "Removed stale socket");
        }

        // 3. Validate the binary exists.
        let resolved_exe = Self::resolve_executable(&config.executable)?;

        // 4. Build and spawn the child process.
        let mut cmd = Command::new(&resolved_exe);
        cmd.args(&config.args);
        cmd.kill_on_drop(true);

        // Set standard env vars for bridge processes.
        cmd.env("ALEPH_INSTANCE_ID", link_id.as_str());
        cmd.env("ALEPH_SOCKET_PATH", socket_path.to_string_lossy().as_ref());
        cmd.env("ALEPH_LOG_LEVEL", "info");

        // Set user-provided env vars.
        for (key, value) in &config.env_vars {
            cmd.env(key, value);
        }

        // Configure stdio for StdioTransport.
        if config.transport_type == TransportType::Stdio {
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::inherit());
        } else {
            cmd.stdin(std::process::Stdio::null());
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        }

        let mut child = cmd.spawn().map_err(|e| {
            BridgeSupervisorError::SpawnFailed(format!(
                "Failed to spawn {}: {e}",
                resolved_exe.display()
            ))
        })?;

        info!(
            link_id = %link_id,
            exe = %resolved_exe.display(),
            "Bridge process spawned"
        );

        // 5. Create transport and connect.
        let transport: Arc<dyn Transport> = match config.transport_type {
            TransportType::UnixSocket => {
                // Wait for the bridge to create its socket.
                Self::wait_for_socket(&socket_path, Duration::from_secs(10)).await?;

                let transport = UnixSocketTransport::new(&socket_path);
                transport.connect(3, 500).await.map_err(|e| {
                    BridgeSupervisorError::ConnectionFailed(format!(
                        "Unix socket connect failed: {e}"
                    ))
                })?;
                Arc::new(transport)
            }
            TransportType::Stdio => {
                let stdin = child.stdin.take().ok_or_else(|| {
                    BridgeSupervisorError::SpawnFailed(
                        "Failed to capture child stdin".to_string(),
                    )
                })?;
                let stdout = child.stdout.take().ok_or_else(|| {
                    BridgeSupervisorError::SpawnFailed(
                        "Failed to capture child stdout".to_string(),
                    )
                })?;
                let transport = StdioTransport::from_child(stdin, stdout);
                transport.set_connected();
                Arc::new(transport)
            }
        };

        // 6. Perform handshake.
        let handshake_result = transport
            .request(
                "aleph.handshake",
                serde_json::json!({
                    "protocol_version": "1.0",
                    "link_id": link_id.as_str(),
                }),
            )
            .await;

        let handshake_response = match handshake_result {
            Ok(response) => {
                debug!(
                    link_id = %link_id,
                    response = %response,
                    "Bridge handshake succeeded"
                );
                response
            }
            Err(e) => {
                // Clean up: kill child and close transport on handshake failure.
                let _ = transport.close().await;
                // child is kill_on_drop, so dropping it will kill the process
                return Err(BridgeSupervisorError::HandshakeFailed(format!(
                    "Handshake with bridge failed: {e}"
                )));
            }
        };

        // 7. Store the managed process — guard against duplicate link_id.
        let health_interval = config.health_check_interval;
        let managed = ManagedProcess {
            link_id: link_id.clone(),
            child,
            transport: Arc::clone(&transport),
            config,
            restart_count: 0,
            last_heartbeat: Instant::now(),
            status: ProcessStatus::Running,
        };

        {
            let mut guard = self.processes.write().await;
            if guard.contains_key(link_id) {
                // Drop `managed` here — kill_on_drop will reap the just-spawned child.
                return Err(BridgeSupervisorError::SpawnFailed(format!(
                    "A process is already managed for link_id '{}'; call stop() first",
                    link_id
                )));
            }
            guard.insert(link_id.clone(), managed);
        }

        // 8. Start health monitor.
        self.start_health_monitor(link_id.clone(), health_interval);

        Ok(SpawnResult {
            transport,
            handshake_response,
        })
    }

    /// Stop a managed bridge process by link id.
    ///
    /// Removes the process from the map, closes the transport, kills the
    /// child process, and cleans up the socket file (if applicable).
    pub async fn stop(&self, link_id: &LinkId) -> Result<(), BridgeSupervisorError> {
        // Remove from the map first.
        let mut managed = {
            let mut guard = self.processes.write().await;
            guard.remove(link_id)
        };

        let Some(ref mut proc) = managed else {
            return Ok(()); // Not found — idempotent stop.
        };

        // Close transport (best-effort).
        if let Err(e) = proc.transport.close().await {
            warn!(
                link_id = %link_id,
                error = %e,
                "Error closing transport during stop"
            );
        }

        // Kill child process (best-effort).
        if let Err(e) = proc.child.kill().await {
            warn!(
                link_id = %link_id,
                error = %e,
                "Error killing child process during stop"
            );
        }

        // Clean up socket file.
        let socket_path = self.run_dir.join(format!("{}.sock", link_id.as_str()));
        if socket_path.exists() {
            let _ = tokio::fs::remove_file(&socket_path).await;
            debug!(
                link_id = %link_id,
                path = %socket_path.display(),
                "Cleaned up socket file"
            );
        }

        info!(link_id = %link_id, "Bridge process stopped");
        Ok(())
    }

    /// Stop all managed bridge processes.
    pub async fn stop_all(&self) {
        // Collect keys first to avoid holding the lock while stopping.
        let link_ids: Vec<LinkId> = {
            let guard = self.processes.read().await;
            guard.keys().cloned().collect()
        };

        for link_id in link_ids {
            if let Err(e) = self.stop(&link_id).await {
                error!(
                    link_id = %link_id,
                    error = %e,
                    "Error stopping bridge process"
                );
            }
        }
    }

    /// Spawn a background health monitor that periodically pings the bridge.
    ///
    /// The monitor sends a `system.ping` request at the configured interval.
    /// On success, updates `last_heartbeat` and sets status to `Running`.
    /// On failure, sets status to `Unhealthy`.
    ///
    /// The monitor stops when the process is removed from the map (i.e. after
    /// `stop()` is called).
    fn start_health_monitor(&self, link_id: LinkId, interval: Duration) {
        let processes = Arc::clone(&self.processes);
        // Clone the shutdown receiver so this task can be cancelled when the
        // supervisor is dropped or stop_all() is called.
        let mut shutdown = self.shutdown_rx.clone();

        tokio::spawn(async move {
            loop {
                // Wait for the next health-check interval, but stop immediately
                // if the shutdown signal is received.
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = shutdown.changed() => {
                        debug!(link_id = %link_id, "Health monitor stopping: shutdown signal received");
                        break;
                    }
                }

                // Read the transport out of the map without holding the lock
                // across the async ping call.
                let transport = {
                    let guard = processes.read().await;
                    guard.get(&link_id).map(|p| Arc::clone(&p.transport))
                };

                let Some(transport) = transport else {
                    // Process was removed from the map — stop monitoring.
                    debug!(
                        link_id = %link_id,
                        "Health monitor stopping: process removed"
                    );
                    break;
                };

                // Ping the bridge.
                let result = transport
                    .request("system.ping", serde_json::json!({}))
                    .await;

                // Now acquire write lock to update status.
                let mut guard = processes.write().await;
                if let Some(proc) = guard.get_mut(&link_id) {
                    match result {
                        Ok(_) => {
                            proc.last_heartbeat = Instant::now();
                            if proc.status == ProcessStatus::Unhealthy {
                                info!(
                                    link_id = %link_id,
                                    "Bridge recovered — marking as Running"
                                );
                            }
                            proc.status = ProcessStatus::Running;
                        }
                        Err(e) => {
                            warn!(
                                link_id = %link_id,
                                error = %e,
                                "Health check failed"
                            );
                            proc.status = ProcessStatus::Unhealthy;
                        }
                    }
                } else {
                    // Process was removed while we were pinging — stop.
                    break;
                }
            }
        });
    }

    /// Wait for a Unix socket file to appear on disk.
    ///
    /// Polls with 100ms sleep intervals until the socket exists or the
    /// timeout is exceeded.
    async fn wait_for_socket(
        path: &std::path::Path,
        timeout: Duration,
    ) -> Result<(), BridgeSupervisorError> {
        let start = Instant::now();
        loop {
            // Use tokio::fs::try_exists to avoid blocking the async thread
            // with a synchronous stat(2) syscall.
            if tokio::fs::try_exists(path).await.unwrap_or(false) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(BridgeSupervisorError::ConnectionFailed(format!(
                    "Socket {} did not appear within {}s",
                    path.display(),
                    timeout.as_secs()
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Resolve an executable path, checking absolute paths exist and looking
    /// up relative paths via `which`.
    fn resolve_executable(executable: &std::path::Path) -> Result<PathBuf, BridgeSupervisorError> {
        if executable.is_absolute() {
            if executable.exists() {
                Ok(executable.to_path_buf())
            } else {
                Err(BridgeSupervisorError::BinaryNotFound(format!(
                    "Executable not found: {}",
                    executable.display()
                )))
            }
        } else {
            // Try to find via `which` for relative/bare names.
            which::which(executable).map_err(|e| {
                BridgeSupervisorError::BinaryNotFound(format!(
                    "Cannot find '{}' in PATH: {e}",
                    executable.display()
                ))
            })
        }
    }
}

impl Drop for BridgeSupervisor {
    fn drop(&mut self) {
        // Signal all background health monitor tasks to stop.
        // kill_on_drop(true) on each Child ensures the OS will reclaim the
        // child processes when the ManagedProcess entries are dropped.
        let _ = self.shutdown_tx.send(true);
        debug!("BridgeSupervisor dropped — signalled health monitors to stop, child processes will be killed via kill_on_drop");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_creation() {
        let supervisor = BridgeSupervisor::new(PathBuf::from("/tmp/aleph-test/run"));
        let processes = supervisor.list_processes();
        assert!(processes.is_empty());
    }

    #[test]
    fn test_managed_process_config() {
        let config = ManagedProcessConfig {
            executable: PathBuf::from("/usr/local/bin/signal-bridge"),
            args: vec!["--verbose".into(), "--port".into(), "9000".into()],
            transport_type: TransportType::UnixSocket,
            max_restarts: 5,
            restart_delay: Duration::from_secs(3),
            health_check_interval: Duration::from_secs(30),
            env_vars: HashMap::from([("MY_VAR".into(), "value".into())]),
        };

        assert_eq!(
            config.executable,
            PathBuf::from("/usr/local/bin/signal-bridge")
        );
        assert_eq!(config.args.len(), 3);
        assert_eq!(config.transport_type, TransportType::UnixSocket);
        assert_eq!(config.max_restarts, 5);
        assert_eq!(config.restart_delay, Duration::from_secs(3));
        assert_eq!(config.health_check_interval, Duration::from_secs(30));
        assert_eq!(config.env_vars.get("MY_VAR").unwrap(), "value");
    }

    #[test]
    fn test_managed_process_config_from_runtime_process() {
        let runtime = BridgeRuntime::Process {
            executable: "./my-bridge".to_string(),
            args: vec!["--flag".into()],
            transport: TransportType::Stdio,
            health_check_interval_secs: 15,
            max_restarts: 3,
            restart_delay_secs: 5,
        };

        let config = ManagedProcessConfig::from_runtime(&runtime);
        assert!(config.is_some());

        let config = config.unwrap();
        assert_eq!(config.executable, PathBuf::from("./my-bridge"));
        assert_eq!(config.args, vec!["--flag"]);
        assert_eq!(config.transport_type, TransportType::Stdio);
        assert_eq!(config.max_restarts, 3);
        assert_eq!(config.restart_delay, Duration::from_secs(5));
        assert_eq!(config.health_check_interval, Duration::from_secs(15));
        assert!(config.env_vars.is_empty());
    }

    #[test]
    fn test_managed_process_config_from_runtime_builtin() {
        let runtime = BridgeRuntime::Builtin;
        let config = ManagedProcessConfig::from_runtime(&runtime);
        assert!(config.is_none());
    }

    #[test]
    fn test_process_status_variants() {
        let starting = ProcessStatus::Starting;
        let running = ProcessStatus::Running;
        let unhealthy = ProcessStatus::Unhealthy;
        let restarting = ProcessStatus::Restarting;
        let stopped = ProcessStatus::Stopped;
        let failed = ProcessStatus::Failed("out of memory".into());

        // PartialEq checks
        assert_eq!(starting, ProcessStatus::Starting);
        assert_eq!(running, ProcessStatus::Running);
        assert_eq!(unhealthy, ProcessStatus::Unhealthy);
        assert_eq!(restarting, ProcessStatus::Restarting);
        assert_eq!(stopped, ProcessStatus::Stopped);
        assert_eq!(failed, ProcessStatus::Failed("out of memory".into()));

        // Different variants are not equal.
        assert_ne!(starting, running);
        assert_ne!(running, unhealthy);
        assert_ne!(
            ProcessStatus::Failed("a".into()),
            ProcessStatus::Failed("b".into())
        );
    }

    #[test]
    fn test_supervisor_error_display() {
        let err = BridgeSupervisorError::BinaryNotFound("signal-bridge".into());
        assert_eq!(err.to_string(), "Binary not found: signal-bridge");

        let err = BridgeSupervisorError::SpawnFailed("permission denied".into());
        assert_eq!(err.to_string(), "Spawn failed: permission denied");

        let err = BridgeSupervisorError::ConnectionFailed("timeout".into());
        assert_eq!(err.to_string(), "Connection failed: timeout");

        let err = BridgeSupervisorError::HandshakeFailed("version mismatch".into());
        assert_eq!(err.to_string(), "Handshake failed: version mismatch");

        let err = BridgeSupervisorError::IoError("disk full".into());
        assert_eq!(err.to_string(), "IO error: disk full");

        let err = BridgeSupervisorError::MaxRestartsExceeded(5);
        assert_eq!(err.to_string(), "Max restarts exceeded: 5");
    }

    #[tokio::test]
    async fn test_list_processes_async_empty() {
        let supervisor = BridgeSupervisor::new(PathBuf::from("/tmp/aleph-test/run"));
        let processes = supervisor.list_processes_async().await;
        assert!(processes.is_empty());
    }

    #[test]
    fn test_resolve_executable_absolute_not_found() {
        let result =
            BridgeSupervisor::resolve_executable(&PathBuf::from("/nonexistent/path/to/binary"));
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeSupervisorError::BinaryNotFound(msg) => {
                assert!(msg.contains("/nonexistent/path/to/binary"));
            }
            other => panic!("Expected BinaryNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn test_resolve_executable_relative_not_found() {
        let result = BridgeSupervisor::resolve_executable(&PathBuf::from(
            "definitely-not-a-real-binary-xyzzy",
        ));
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeSupervisorError::BinaryNotFound(msg) => {
                assert!(msg.contains("definitely-not-a-real-binary-xyzzy"));
            }
            other => panic!("Expected BinaryNotFound, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_stop_nonexistent_link() {
        let supervisor = BridgeSupervisor::new(PathBuf::from("/tmp/aleph-test/run"));
        // Stopping a link that doesn't exist should be idempotent.
        let result = supervisor.stop(&LinkId::new("nonexistent")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stop_all_empty() {
        let supervisor = BridgeSupervisor::new(PathBuf::from("/tmp/aleph-test/run"));
        // stop_all on empty supervisor should not panic.
        supervisor.stop_all().await;
    }
}

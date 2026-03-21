//! Server Manager — manages the embedded aleph lifecycle.
//!
//! On Linux/Windows, the Tauri app bundles the `aleph` binary as a
//! resource. `ServerManager` spawns it as a child process, waits for the
//! UDS/named-pipe socket to become ready, and ensures graceful shutdown on
//! drop.

use std::path::PathBuf;
use std::process::{Child, Command};
use tracing::{error, info, warn};

/// Manages the lifecycle of an embedded aleph process.
pub struct ServerManager {
    process: Option<Child>,
    socket_path: PathBuf,
}

impl ServerManager {
    /// Create a new `ServerManager` targeting the given socket path.
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            process: None,
            socket_path,
        }
    }

    /// Start the aleph binary from the given resource directory.
    ///
    /// The binary is expected at `resource_dir/aleph` (Unix) or
    /// `resource_dir/aleph.exe` (Windows).
    pub fn start(&mut self, resource_dir: &std::path::Path) -> Result<(), String> {
        let server_bin = if cfg!(target_os = "windows") {
            resource_dir.join("aleph.exe")
        } else {
            resource_dir.join("aleph")
        };

        if !server_bin.exists() {
            return Err(format!("Server binary not found at {:?}", server_bin));
        }

        info!("Starting aleph from {:?}", server_bin);

        // Ensure socket parent directory exists
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Remove stale socket file
        std::fs::remove_file(&self.socket_path).ok();

        let child = Command::new(&server_bin)
            .args([
                "--bridge-mode",
                "--socket",
                &self.socket_path.to_string_lossy(),
            ])
            .spawn()
            .map_err(|e| format!("Failed to start server: {}", e))?;

        info!("aleph started (PID: {})", child.id());
        self.process = Some(child);
        self.wait_for_ready()
    }

    /// Stop the aleph process gracefully (SIGTERM on Unix, kill on Windows).
    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            info!("Stopping aleph");

            // Send SIGTERM on Unix, hard kill on Windows
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
            }
            #[cfg(windows)]
            {
                child.kill().ok();
            }

            // Wait up to 3 seconds for graceful exit
            std::thread::sleep(std::time::Duration::from_secs(3));
            match child.try_wait() {
                Ok(Some(status)) => info!("Server stopped with status: {}", status),
                Ok(None) => {
                    warn!("Server did not exit gracefully, force killing");
                    child.kill().ok();
                    child.wait().ok();
                }
                Err(e) => {
                    error!("Error waiting for server: {}", e);
                    child.kill().ok();
                    child.wait().ok();
                }
            }
        }

        // Clean up socket file
        std::fs::remove_file(&self.socket_path).ok();
    }

    /// Wait for the server to create the socket and accept connections.
    /// Polls every 200ms for up to 10 seconds.
    fn wait_for_ready(&self) -> Result<(), String> {
        for _ in 0..50 {
            if self.socket_path.exists() {
                #[cfg(unix)]
                {
                    if std::os::unix::net::UnixStream::connect(&self.socket_path).is_ok() {
                        info!("Server is ready at {:?}", self.socket_path);
                        return Ok(());
                    }
                }
                #[cfg(windows)]
                {
                    // On Windows, named pipes work differently; if the path exists
                    // we consider the server ready.
                    info!("Server is ready at {:?}", self.socket_path);
                    return Ok(());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err("Server did not become ready within 10 seconds".into())
    }
}

impl Drop for ServerManager {
    fn drop(&mut self) {
        self.stop();
    }
}

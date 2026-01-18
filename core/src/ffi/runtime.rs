//! Runtime management FFI module
//!
//! Provides FFI wrappers for the runtime management system,
//! exposing functionality to install, update, and manage external runtimes
//! like uv (Python), fnm (Node.js), and yt-dlp.

use super::{AetherCore, AetherFfiError};
use crate::runtimes::{FnmRuntime, RuntimeManager, RuntimeRegistry, UvRuntime};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Runtime information for FFI (UniFFI dictionary)
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    /// Runtime ID (e.g., "fnm", "uv", "yt-dlp")
    pub id: String,
    /// Human-readable name (e.g., "fnm (Node.js)")
    pub name: String,
    /// Description of the runtime
    pub description: String,
    /// Installed version (None if not installed)
    pub version: Option<String>,
    /// Whether the runtime is installed
    pub installed: bool,
}

impl From<crate::runtimes::RuntimeInfo> for RuntimeInfo {
    fn from(info: crate::runtimes::RuntimeInfo) -> Self {
        Self {
            id: info.id.to_string(),
            name: info.name.to_string(),
            description: info.description.to_string(),
            version: info.version,
            installed: info.installed,
        }
    }
}

/// Update information for FFI (UniFFI dictionary)
#[derive(Debug, Clone)]
pub struct RuntimeUpdateInfo {
    /// Runtime ID
    pub runtime_id: String,
    /// Currently installed version
    pub current_version: String,
    /// Latest available version
    pub latest_version: String,
}

impl From<crate::runtimes::UpdateInfo> for RuntimeUpdateInfo {
    fn from(info: crate::runtimes::UpdateInfo) -> Self {
        Self {
            runtime_id: info.runtime_id,
            current_version: info.current_version,
            latest_version: info.latest_version,
        }
    }
}

impl AetherCore {
    /// List all registered runtimes with their status
    pub fn list_runtimes(&self) -> Vec<RuntimeInfo> {
        match RuntimeRegistry::new() {
            Ok(registry) => registry
                .list()
                .into_iter()
                .map(RuntimeInfo::from)
                .collect(),
            Err(e) => {
                debug!(error = %e, "Failed to create RuntimeRegistry");
                Vec::new()
            }
        }
    }

    /// Check if a specific runtime is installed
    pub fn is_runtime_installed(&self, runtime_id: String) -> bool {
        match RuntimeRegistry::new() {
            Ok(registry) => registry.is_installed(&runtime_id),
            Err(e) => {
                debug!(error = %e, "Failed to check runtime installation status");
                false
            }
        }
    }

    /// Install a runtime (lazy installation)
    ///
    /// Returns immediately if already installed
    pub fn install_runtime(&self, runtime_id: String) -> Result<(), AetherFfiError> {
        info!(runtime_id = %runtime_id, "Installing runtime");

        let registry = RuntimeRegistry::new()
            .map_err(|e| AetherFfiError::Config(format!("Failed to create registry: {}", e)))?;

        // Use the runtime handle to run async code
        self.runtime.block_on(async {
            registry
                .require(&runtime_id)
                .await
                .map_err(|e| AetherFfiError::Config(format!("Failed to install runtime: {}", e)))?;
            Ok(())
        })
    }

    /// Check for available updates across all runtimes
    ///
    /// Returns list of runtimes with available updates
    pub fn check_runtime_updates(&self) -> Result<Vec<RuntimeUpdateInfo>, AetherFfiError> {
        info!("Checking for runtime updates");

        let registry = RuntimeRegistry::new()
            .map_err(|e| AetherFfiError::Config(format!("Failed to create registry: {}", e)))?;

        self.runtime.block_on(async {
            let updates = registry.check_updates().await;
            Ok(updates.into_iter().map(RuntimeUpdateInfo::from).collect())
        })
    }

    /// Update a specific runtime to latest version
    pub fn update_runtime(&self, runtime_id: String) -> Result<(), AetherFfiError> {
        info!(runtime_id = %runtime_id, "Updating runtime");

        let registry = RuntimeRegistry::new()
            .map_err(|e| AetherFfiError::Config(format!("Failed to create registry: {}", e)))?;

        self.runtime.block_on(async {
            registry
                .update(&runtime_id)
                .await
                .map_err(|e| AetherFfiError::Config(format!("Failed to update runtime: {}", e)))
        })
    }

    /// Get Node.js runtime path (via fnm)
    ///
    /// Returns None if fnm/Node.js is not installed
    pub fn get_node_path(&self) -> Option<String> {
        let runtimes_dir = crate::runtimes::get_runtimes_dir().ok()?;
        let fnm = FnmRuntime::new(runtimes_dir);
        if fnm.is_installed() {
            Some(fnm.node_path().to_string_lossy().to_string())
        } else {
            None
        }
    }

    /// Get npm runtime path (via fnm)
    ///
    /// Returns None if fnm/Node.js is not installed
    pub fn get_npm_path(&self) -> Option<String> {
        let runtimes_dir = crate::runtimes::get_runtimes_dir().ok()?;
        let fnm = FnmRuntime::new(runtimes_dir);
        if fnm.is_installed() {
            Some(fnm.npm_path().to_string_lossy().to_string())
        } else {
            None
        }
    }

    /// Get Python runtime path (via uv)
    ///
    /// Returns None if uv/Python is not installed
    pub fn get_python_path(&self) -> Option<String> {
        let runtimes_dir = crate::runtimes::get_runtimes_dir().ok()?;
        let uv = UvRuntime::new(runtimes_dir);
        if uv.is_installed() {
            Some(uv.python_path().to_string_lossy().to_string())
        } else {
            None
        }
    }

    /// Get yt-dlp runtime path
    ///
    /// Returns None if yt-dlp is not installed
    pub fn get_ytdlp_path(&self) -> Option<String> {
        match RuntimeRegistry::new() {
            Ok(registry) => {
                if registry.is_installed("yt-dlp") {
                    registry
                        .get("yt-dlp")
                        .map(|rt| rt.executable_path().to_string_lossy().to_string())
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    /// Start background runtime update check
    ///
    /// This spawns an async task that checks for updates and notifies
    /// the UI via callback if updates are available.
    /// Called automatically during AetherCore initialization.
    pub(crate) fn start_runtime_update_check(&self) {
        let handler = Arc::clone(&self.handler);

        // Spawn background task for update check
        self.runtime.spawn(async move {
            // Create registry for update check
            let registry = match RuntimeRegistry::new() {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "Failed to create RuntimeRegistry for update check");
                    return;
                }
            };

            // Check if we should perform update check (24-hour throttle)
            if !registry.should_check_updates().await {
                debug!("Skipping runtime update check (within throttle interval)");
                return;
            }

            info!("Starting background runtime update check");

            // Perform update check
            let updates = registry.check_updates().await;

            if !updates.is_empty() {
                info!(
                    count = updates.len(),
                    "Runtime updates available"
                );

                // Convert to FFI types and notify UI
                let ffi_updates: Vec<RuntimeUpdateInfo> = updates
                    .into_iter()
                    .map(RuntimeUpdateInfo::from)
                    .collect();

                handler.on_runtime_updates_available(ffi_updates);
            } else {
                debug!("All runtimes are up to date");
            }
        });
    }
}

//! RuntimeRegistry - Central management point for all runtimes
//!
//! The registry is the main entry point for runtime operations.
//! It handles lazy installation, update checking, and provides
//! unified access to all runtime implementations.

use super::manager::{RuntimeInfo, RuntimeManager, UpdateInfo};
use super::manifest::Manifest;
use super::{get_runtimes_dir, FfmpegRuntime, FnmRuntime, UvRuntime, YtDlpRuntime};
use crate::error::{AetherError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Central registry for all runtime managers
///
/// Provides:
/// - Lazy installation via `require()`
/// - Runtime listing for UI
/// - Background update checking
pub struct RuntimeRegistry {
    /// All known runtimes indexed by ID
    runtimes: HashMap<&'static str, Arc<dyn RuntimeManager>>,
    /// Persistent metadata storage
    manifest: RwLock<Manifest>,
    /// Base directory for all runtimes
    runtimes_dir: PathBuf,
}

impl RuntimeRegistry {
    /// Create a new registry with all known runtimes
    ///
    /// This will:
    /// 1. Create the runtimes directory if needed
    /// 2. Load or create the manifest
    /// 3. Register all known runtime implementations
    /// 4. Run any necessary migrations
    pub fn new() -> Result<Self> {
        let runtimes_dir = get_runtimes_dir()?;

        // Ensure directory exists
        std::fs::create_dir_all(&runtimes_dir).map_err(|e| {
            AetherError::runtime(
                "registry",
                format!("Failed to create runtimes directory: {}", e),
            )
        })?;

        // Load manifest
        let manifest = Manifest::load_or_default(&runtimes_dir)?;

        // Create runtime instances
        let mut runtimes: HashMap<&'static str, Arc<dyn RuntimeManager>> = HashMap::new();

        let ffmpeg = Arc::new(FfmpegRuntime::new(runtimes_dir.clone()));
        let ytdlp = Arc::new(YtDlpRuntime::new(runtimes_dir.clone()));
        let uv = Arc::new(UvRuntime::new(runtimes_dir.clone()));
        let fnm = Arc::new(FnmRuntime::new(runtimes_dir.clone()));

        // Run migrations for each runtime
        if let Err(e) = ytdlp.migrate_if_needed() {
            warn!("Failed to migrate yt-dlp: {}", e);
        }

        runtimes.insert(ffmpeg.id(), ffmpeg);
        runtimes.insert(ytdlp.id(), ytdlp);
        runtimes.insert(uv.id(), uv);
        runtimes.insert(fnm.id(), fnm);

        info!(
            runtimes_dir = ?runtimes_dir,
            count = runtimes.len(),
            "RuntimeRegistry initialized"
        );

        Ok(Self {
            runtimes,
            manifest: RwLock::new(manifest),
            runtimes_dir,
        })
    }

    /// Get a runtime by ID
    ///
    /// Returns the runtime without checking installation status.
    pub fn get(&self, id: &str) -> Option<Arc<dyn RuntimeManager>> {
        self.runtimes.get(id).cloned()
    }

    /// Get a runtime, installing it if necessary
    ///
    /// This is the primary way to access runtimes. It ensures the
    /// runtime is installed before returning.
    pub async fn require(&self, id: &str) -> Result<Arc<dyn RuntimeManager>> {
        let runtime = self.runtimes.get(id).ok_or_else(|| {
            AetherError::runtime("registry", format!("Unknown runtime: {}", id))
        })?;

        if !runtime.is_installed() {
            info!(runtime_id = %id, "Runtime not installed, installing...");

            runtime.install().await?;

            // Update manifest
            let version = runtime.get_version().unwrap_or_else(|| "unknown".to_string());
            self.manifest.write().await.mark_installed(id, version)?;

            info!(runtime_id = %id, "Runtime installed successfully");
        }

        Ok(Arc::clone(runtime))
    }

    /// List all known runtimes with their status
    pub fn list(&self) -> Vec<RuntimeInfo> {
        self.runtimes.values().map(|r| r.info()).collect()
    }

    /// Check if a specific runtime is installed
    pub fn is_installed(&self, id: &str) -> bool {
        self.runtimes
            .get(id)
            .map(|r| r.is_installed())
            .unwrap_or(false)
    }

    /// Check for updates on all installed runtimes
    ///
    /// Returns a list of available updates. This is designed to be
    /// called in the background at startup.
    pub async fn check_updates(&self) -> Vec<UpdateInfo> {
        let mut updates = Vec::new();

        // Check if we should run update checks
        {
            let manifest = self.manifest.read().await;
            if !manifest.should_check_updates() {
                debug!("Skipping update check (within interval)");
                return updates;
            }
        }

        info!("Checking for runtime updates...");

        for (id, runtime) in &self.runtimes {
            if !runtime.is_installed() {
                continue;
            }

            if let Some(update) = runtime.check_update().await {
                info!(
                    runtime_id = %id,
                    current = %update.current_version,
                    latest = %update.latest_version,
                    "Update available"
                );
                updates.push(update);
            }

            // Mark as checked regardless of result
            let mut manifest = self.manifest.write().await;
            if let Err(e) = manifest.mark_update_checked(id) {
                warn!("Failed to update manifest: {}", e);
            }
        }

        updates
    }

    /// Update a specific runtime to the latest version
    pub async fn update(&self, id: &str) -> Result<()> {
        let runtime = self.runtimes.get(id).ok_or_else(|| {
            AetherError::runtime("registry", format!("Unknown runtime: {}", id))
        })?;

        if !runtime.is_installed() {
            return Err(AetherError::runtime(
                id,
                "Cannot update: runtime is not installed",
            ));
        }

        info!(runtime_id = %id, "Updating runtime...");
        runtime.update().await?;

        // Update manifest with new version
        let version = runtime.get_version().unwrap_or_else(|| "unknown".to_string());
        self.manifest.write().await.update_version(id, version)?;

        info!(runtime_id = %id, "Runtime updated successfully");
        Ok(())
    }

    /// Get the runtimes directory path
    pub fn runtimes_dir(&self) -> &PathBuf {
        &self.runtimes_dir
    }

    /// Check if update checks should be performed
    pub async fn should_check_updates(&self) -> bool {
        self.manifest.read().await.should_check_updates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        // This test requires HOME to be set
        if std::env::var("HOME").is_err() {
            return;
        }

        let registry = RuntimeRegistry::new();
        assert!(registry.is_ok());

        let registry = registry.unwrap();
        let runtimes = registry.list();
        assert_eq!(runtimes.len(), 4); // ffmpeg, yt-dlp, uv, fnm
    }

    #[test]
    fn test_get_unknown_runtime() {
        if std::env::var("HOME").is_err() {
            return;
        }

        let registry = RuntimeRegistry::new().unwrap();
        assert!(registry.get("unknown-runtime").is_none());
    }
}

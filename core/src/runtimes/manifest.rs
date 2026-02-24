//! Manifest for runtime metadata persistence
//!
//! Stores installation timestamps, versions, and update check times
//! in `~/.aleph/runtimes/manifest.json`.

use crate::error::{AlephError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{debug, warn};

/// Manifest file version for future migrations
#[allow(dead_code)]
const MANIFEST_VERSION: u32 = 1;

/// Minimum interval between update checks (24 hours)
#[allow(dead_code)]
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Metadata for a single runtime
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct RuntimeMetadata {
    /// When the runtime was installed
    pub installed_at: SystemTime,
    /// Installed version string
    pub version: String,
    /// When we last checked for updates
    #[serde(default)]
    pub last_update_check: Option<SystemTime>,
    /// Additional runtime-specific metadata
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

#[allow(dead_code)]
impl RuntimeMetadata {
    /// Create new metadata for a freshly installed runtime
    pub fn new(version: String) -> Self {
        Self {
            installed_at: SystemTime::now(),
            version,
            last_update_check: None,
            extra: HashMap::new(),
        }
    }

    /// Update the version after an upgrade
    pub fn update_version(&mut self, version: String) {
        self.version = version;
        self.installed_at = SystemTime::now();
    }

    /// Mark that we just checked for updates
    pub fn mark_update_checked(&mut self) {
        self.last_update_check = Some(SystemTime::now());
    }
}

/// Manifest storing metadata for all runtimes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Manifest {
    /// Manifest format version
    pub version: u32,
    /// Per-runtime metadata
    pub runtimes: HashMap<String, RuntimeMetadata>,
    /// Path to the manifest file (not serialized)
    #[serde(skip)]
    path: PathBuf,
}

#[allow(dead_code)]
impl Manifest {
    /// Load manifest from disk, or create a new one if it doesn't exist
    pub fn load_or_default(runtimes_dir: &Path) -> Result<Self> {
        let path = runtimes_dir.join("manifest.json");

        if path.exists() {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                AlephError::runtime("manifest", format!("Failed to read manifest: {}", e))
            })?;

            let mut manifest: Manifest = serde_json::from_str(&content).map_err(|e| {
                warn!("Failed to parse manifest, creating new one: {}", e);
                AlephError::runtime("manifest", format!("Failed to parse manifest: {}", e))
            })?;

            manifest.path = path;

            // Handle version migrations if needed
            if manifest.version < MANIFEST_VERSION {
                debug!(
                    old_version = manifest.version,
                    new_version = MANIFEST_VERSION,
                    "Migrating manifest"
                );
                manifest.version = MANIFEST_VERSION;
                manifest.save()?;
            }

            Ok(manifest)
        } else {
            debug!("Creating new manifest at {:?}", path);
            Ok(Self {
                version: MANIFEST_VERSION,
                runtimes: HashMap::new(),
                path,
            })
        }
    }

    /// Save manifest to disk
    pub fn save(&self) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::runtime(
                    "manifest",
                    format!("Failed to create manifest directory: {}", e),
                )
            })?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|e| {
            AlephError::runtime("manifest", format!("Failed to serialize manifest: {}", e))
        })?;

        std::fs::write(&self.path, content).map_err(|e| {
            AlephError::runtime("manifest", format!("Failed to write manifest: {}", e))
        })?;

        debug!("Saved manifest to {:?}", self.path);
        Ok(())
    }

    /// Mark a runtime as installed with the given version
    pub fn mark_installed(&mut self, id: &str, version: String) -> Result<()> {
        self.runtimes
            .insert(id.to_string(), RuntimeMetadata::new(version));
        self.save()
    }

    /// Update the version for a runtime
    pub fn update_version(&mut self, id: &str, version: String) -> Result<()> {
        if let Some(meta) = self.runtimes.get_mut(id) {
            meta.update_version(version);
            self.save()?;
        }
        Ok(())
    }

    /// Mark that we checked for updates for a runtime
    pub fn mark_update_checked(&mut self, id: &str) -> Result<()> {
        if let Some(meta) = self.runtimes.get_mut(id) {
            meta.mark_update_checked();
            self.save()?;
        }
        Ok(())
    }

    /// Get metadata for a runtime
    pub fn get(&self, id: &str) -> Option<&RuntimeMetadata> {
        self.runtimes.get(id)
    }

    /// Check if we should perform update checks
    ///
    /// Returns true if any installed runtime hasn't been checked
    /// within the update check interval.
    pub fn should_check_updates(&self) -> bool {
        let now = SystemTime::now();

        for meta in self.runtimes.values() {
            match meta.last_update_check {
                Some(last_check) => {
                    if let Ok(elapsed) = now.duration_since(last_check) {
                        if elapsed > UPDATE_CHECK_INTERVAL {
                            return true;
                        }
                    }
                }
                None => return true,
            }
        }

        false
    }

    /// Get the version of an installed runtime
    pub fn get_version(&self, id: &str) -> Option<String> {
        self.runtimes.get(id).map(|m| m.version.clone())
    }

    /// Remove a runtime from the manifest
    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.runtimes.remove(id);
        self.save()
    }

    /// Store extra metadata for a runtime
    pub fn set_extra(&mut self, id: &str, key: &str, value: String) -> Result<()> {
        if let Some(meta) = self.runtimes.get_mut(id) {
            meta.extra.insert(key.to_string(), value);
            self.save()?;
        }
        Ok(())
    }

    /// Get extra metadata for a runtime
    pub fn get_extra(&self, id: &str, key: &str) -> Option<&String> {
        self.runtimes.get(id).and_then(|m| m.extra.get(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_manifest_create_and_save() {
        let temp_dir = TempDir::new().unwrap();
        let runtimes_dir = temp_dir.path();

        let mut manifest = Manifest::load_or_default(runtimes_dir).unwrap();
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert!(manifest.runtimes.is_empty());

        manifest
            .mark_installed("yt-dlp", "2024.12.23".to_string())
            .unwrap();
        assert!(manifest.get("yt-dlp").is_some());

        // Reload and verify
        let reloaded = Manifest::load_or_default(runtimes_dir).unwrap();
        assert_eq!(
            reloaded.get_version("yt-dlp"),
            Some("2024.12.23".to_string())
        );
    }

    #[test]
    fn test_should_check_updates() {
        let temp_dir = TempDir::new().unwrap();
        let runtimes_dir = temp_dir.path();

        let mut manifest = Manifest::load_or_default(runtimes_dir).unwrap();

        // No runtimes = no need to check
        assert!(!manifest.should_check_updates());

        // Add runtime without update check
        manifest
            .mark_installed("yt-dlp", "1.0.0".to_string())
            .unwrap();
        assert!(manifest.should_check_updates());

        // Mark as checked
        manifest.mark_update_checked("yt-dlp").unwrap();
        assert!(!manifest.should_check_updates());
    }
}

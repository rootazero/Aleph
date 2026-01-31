//! Storage layer for exec approvals with optimistic locking.
//!
//! Provides file-based persistence with SHA256 hash-based optimistic concurrency control.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

use super::config::ExecApprovalsFile;

/// Storage errors
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Optimistic lock failed: config changed since last load (base: {base}, current: {current})")]
    OptimisticLockFailed { base: String, current: String },

    #[error("Config file not found: {0}")]
    NotFound(PathBuf),
}

/// Result of loading config with hash
#[derive(Debug, Clone)]
pub struct ConfigWithHash {
    pub config: ExecApprovalsFile,
    pub hash: String,
}

/// Storage for exec approvals configuration
pub struct ExecApprovalsStorage {
    path: PathBuf,
}

impl ExecApprovalsStorage {
    /// Create storage with default path (~/.aether/exec-approvals.json)
    pub fn new() -> Self {
        Self::with_path(Self::default_path())
    }

    /// Create storage with custom path
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Get default config path
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aether")
            .join("exec-approvals.json")
    }

    /// Get the config path
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load config and return with hash
    pub fn load(&self) -> Result<ConfigWithHash, StorageError> {
        if !self.path.exists() {
            // Return default config with empty hash
            let config = ExecApprovalsFile::default();
            let json = serde_json::to_string_pretty(&config)?;
            let hash = compute_hash(&json);
            return Ok(ConfigWithHash { config, hash });
        }

        let content = fs::read_to_string(&self.path)?;
        let config: ExecApprovalsFile = serde_json::from_str(&content)?;
        let hash = compute_hash(&content);

        Ok(ConfigWithHash { config, hash })
    }

    /// Load config only (without hash)
    pub fn load_config(&self) -> Result<ExecApprovalsFile, StorageError> {
        Ok(self.load()?.config)
    }

    /// Compute current hash without loading full config
    pub fn current_hash(&self) -> Result<String, StorageError> {
        if !self.path.exists() {
            let config = ExecApprovalsFile::default();
            let json = serde_json::to_string_pretty(&config)?;
            return Ok(compute_hash(&json));
        }

        let content = fs::read_to_string(&self.path)?;
        Ok(compute_hash(&content))
    }

    /// Save config with optimistic locking
    ///
    /// # Arguments
    ///
    /// * `config` - The config to save
    /// * `base_hash` - Hash from when config was loaded (for optimistic lock)
    ///
    /// # Returns
    ///
    /// New hash after save, or error if lock failed
    pub fn save(&self, config: &ExecApprovalsFile, base_hash: &str) -> Result<String, StorageError> {
        // Check optimistic lock
        let current_hash = self.current_hash()?;
        if current_hash != base_hash {
            return Err(StorageError::OptimisticLockFailed {
                base: base_hash.to_string(),
                current: current_hash,
            });
        }

        self.save_unchecked(config)
    }

    /// Save config without optimistic lock check
    ///
    /// Use this for initial creation or when you know you have exclusive access.
    pub fn save_unchecked(&self, config: &ExecApprovalsFile) -> Result<String, StorageError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Serialize with pretty print
        let json = serde_json::to_string_pretty(config)?;

        // Write atomically using temp file
        let temp_path = self.path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&temp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temp_path, &self.path)?;

        // Set permissions to 0600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            let _ = fs::set_permissions(&self.path, perms);
        }

        Ok(compute_hash(&json))
    }

    /// Check if config file exists
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Delete config file
    pub fn delete(&self) -> Result<(), StorageError> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

impl Default for ExecApprovalsStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute SHA256 hash of content, returning hex string
fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_storage() -> (TempDir, ExecApprovalsStorage) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("exec-approvals.json");
        let storage = ExecApprovalsStorage::with_path(path);
        (dir, storage)
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let (_dir, storage) = temp_storage();

        let result = storage.load().unwrap();
        assert_eq!(result.config.version, 1);
        assert!(!result.hash.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let (_dir, storage) = temp_storage();

        let mut config = ExecApprovalsFile::default();
        config.defaults = Some(super::super::config::ExecDefaults {
            security: Some(super::super::config::ExecSecurity::Allowlist),
            ..Default::default()
        });

        // First save (no base hash needed for unchecked)
        let hash1 = storage.save_unchecked(&config).unwrap();
        assert!(!hash1.is_empty());

        // Load and verify
        let loaded = storage.load().unwrap();
        assert!(loaded.config.defaults.is_some());
        assert_eq!(loaded.hash, hash1);
    }

    #[test]
    fn test_optimistic_lock_success() {
        let (_dir, storage) = temp_storage();

        let config = ExecApprovalsFile::default();
        let hash1 = storage.save_unchecked(&config).unwrap();

        // Save with correct base hash
        let hash2 = storage.save(&config, &hash1).unwrap();
        assert!(!hash2.is_empty());
    }

    #[test]
    fn test_optimistic_lock_failure() {
        let (_dir, storage) = temp_storage();

        let config = ExecApprovalsFile::default();
        let _hash1 = storage.save_unchecked(&config).unwrap();

        // Simulate concurrent modification
        let mut modified = config.clone();
        modified.version = 2;
        let _hash2 = storage.save_unchecked(&modified).unwrap();

        // Try to save with stale hash
        let result = storage.save(&config, "stale-hash");
        assert!(matches!(result, Err(StorageError::OptimisticLockFailed { .. })));
    }

    #[test]
    fn test_hash_consistency() {
        let content = r#"{"version":1}"#;
        let hash1 = compute_hash(content);
        let hash2 = compute_hash(content);
        assert_eq!(hash1, hash2);

        let different = r#"{"version":2}"#;
        let hash3 = compute_hash(different);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_exists() {
        let (_dir, storage) = temp_storage();

        assert!(!storage.exists());

        storage.save_unchecked(&ExecApprovalsFile::default()).unwrap();
        assert!(storage.exists());
    }

    #[test]
    fn test_delete() {
        let (_dir, storage) = temp_storage();

        storage.save_unchecked(&ExecApprovalsFile::default()).unwrap();
        assert!(storage.exists());

        storage.delete().unwrap();
        assert!(!storage.exists());
    }
}

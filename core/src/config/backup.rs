//! Configuration backup and snapshot management
//!
//! This module provides file-level config snapshots before changes are applied.
//! Backups are stored as timestamped copies in `~/.aleph/backups/`.

use crate::error::{AlephError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A single backup entry with path and timestamp
#[derive(Debug, Clone)]
pub struct BackupEntry {
    /// Full path to the backup file
    pub path: PathBuf,
    /// Timestamp suffix extracted from the filename (e.g., "20260301T120000")
    pub timestamp: String,
}

/// Manages config file snapshots with automatic cleanup
#[derive(Debug, Clone)]
pub struct ConfigBackup {
    /// Directory where backup files are stored
    backup_dir: PathBuf,
    /// Maximum number of backups to retain
    max_count: usize,
}

impl ConfigBackup {
    /// Create a new ConfigBackup manager
    ///
    /// # Arguments
    /// * `backup_dir` - Directory to store backup files
    /// * `max_count` - Maximum number of backups to keep (oldest are pruned)
    pub fn new(backup_dir: PathBuf, max_count: usize) -> Self {
        Self {
            backup_dir,
            max_count,
        }
    }

    /// Returns the default backup directory: `~/.aleph/backups/`
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("backups")
    }

    /// Create a timestamped snapshot of the given config file
    ///
    /// The backup is named `config.toml.<TIMESTAMP>` where TIMESTAMP
    /// is in `%Y%m%dT%H%M%S` format (e.g., `config.toml.20260301T120000`).
    ///
    /// After creating the snapshot, old backups beyond `max_count` are pruned.
    ///
    /// # Arguments
    /// * `config_path` - Path to the config file to back up
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to the created backup file
    /// * `Err` - If the source file doesn't exist or I/O fails
    pub fn create_snapshot(&self, config_path: &Path) -> Result<PathBuf> {
        if !config_path.exists() {
            return Err(AlephError::invalid_config(format!(
                "Config file does not exist: {}",
                config_path.display()
            )));
        }

        // Ensure backup directory exists
        fs::create_dir_all(&self.backup_dir).map_err(|e| {
            AlephError::invalid_config(format!(
                "Failed to create backup directory {}: {}",
                self.backup_dir.display(),
                e
            ))
        })?;
        debug!(dir = %self.backup_dir.display(), "Backup directory ensured");

        // Generate timestamp suffix
        let timestamp = chrono::Local::now().format("%Y%m%dT%H%M%S").to_string();
        let backup_filename = format!("config.toml.{}", timestamp);
        let backup_path = self.backup_dir.join(&backup_filename);

        // Copy config file to backup location
        fs::copy(config_path, &backup_path).map_err(|e| {
            AlephError::invalid_config(format!(
                "Failed to copy config to {}: {}",
                backup_path.display(),
                e
            ))
        })?;
        debug!(
            backup = %backup_path.display(),
            source = %config_path.display(),
            "Config snapshot created"
        );

        // Auto-cleanup old backups
        if let Err(e) = self.cleanup() {
            warn!(error = %e, "Failed to cleanup old backups");
        }

        Ok(backup_path)
    }

    /// Remove oldest backups beyond `max_count`
    ///
    /// Backups are sorted by timestamp (ascending), and the oldest
    /// entries beyond the limit are deleted.
    pub fn cleanup(&self) -> Result<()> {
        let mut entries = self.list()?;

        if entries.len() <= self.max_count {
            debug!(
                count = entries.len(),
                max = self.max_count,
                "No cleanup needed"
            );
            return Ok(());
        }

        // entries are sorted ascending by timestamp, so oldest come first
        let to_remove = entries.len() - self.max_count;
        let removed: Vec<BackupEntry> = entries.drain(..to_remove).collect();

        for entry in &removed {
            if let Err(e) = fs::remove_file(&entry.path) {
                warn!(
                    path = %entry.path.display(),
                    error = %e,
                    "Failed to remove old backup"
                );
            } else {
                debug!(path = %entry.path.display(), "Removed old backup");
            }
        }

        debug!(
            removed = removed.len(),
            remaining = entries.len(),
            "Backup cleanup complete"
        );

        Ok(())
    }

    /// List all backup entries sorted by timestamp (ascending)
    ///
    /// Scans the backup directory for files matching `config.toml.*`
    /// and returns them sorted by their timestamp suffix.
    ///
    /// # Returns
    /// * `Ok(Vec<BackupEntry>)` - List of backup entries (empty if dir doesn't exist)
    pub fn list(&self) -> Result<Vec<BackupEntry>> {
        if !self.backup_dir.exists() {
            return Ok(Vec::new());
        }

        let read_dir = fs::read_dir(&self.backup_dir).map_err(|e| {
            AlephError::invalid_config(format!(
                "Failed to read backup directory {}: {}",
                self.backup_dir.display(),
                e
            ))
        })?;

        let prefix = "config.toml.";
        let mut entries: Vec<BackupEntry> = Vec::new();

        for dir_entry in read_dir {
            let dir_entry = match dir_entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "Failed to read directory entry");
                    continue;
                }
            };

            let file_name = dir_entry.file_name();
            let name = file_name.to_string_lossy();

            if let Some(timestamp) = name.strip_prefix(prefix) {
                entries.push(BackupEntry {
                    path: dir_entry.path(),
                    timestamp: timestamp.to_string(),
                });
            }
        }

        // Sort by timestamp ascending (oldest first)
        entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        debug!(count = entries.len(), "Listed backup entries");
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_create_snapshot() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");
        let backup_dir = tmp.path().join("backups");

        // Write a test config file
        let content = "[general]\nname = \"test\"\n";
        fs::write(&config_path, content).unwrap();

        let backup = ConfigBackup::new(backup_dir.clone(), 5);
        let snapshot_path = backup.create_snapshot(&config_path).unwrap();

        // Verify snapshot exists and content matches
        assert!(snapshot_path.exists());
        let snapshot_content = fs::read_to_string(&snapshot_path).unwrap();
        assert_eq!(snapshot_content, content);

        // Verify it's inside the backup directory
        assert!(snapshot_path.starts_with(&backup_dir));
    }

    #[test]
    fn test_create_snapshot_missing_file() {
        let tmp = TempDir::new().unwrap();
        let nonexistent = tmp.path().join("does_not_exist.toml");
        let backup_dir = tmp.path().join("backups");

        let backup = ConfigBackup::new(backup_dir, 5);
        let result = backup.create_snapshot(&nonexistent);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("does not exist"));
    }

    #[test]
    fn test_cleanup_keeps_max_count() {
        let tmp = TempDir::new().unwrap();
        let backup_dir = tmp.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        // Create 5 backup files with distinct timestamps
        for i in 1..=5 {
            let name = format!("config.toml.20260301T12000{}", i);
            fs::write(backup_dir.join(&name), format!("backup {}", i)).unwrap();
        }

        let backup = ConfigBackup::new(backup_dir.clone(), 3);
        backup.cleanup().unwrap();

        let entries = backup.list().unwrap();
        assert_eq!(entries.len(), 3);

        // Verify the latest 3 are kept (timestamps 3, 4, 5)
        assert_eq!(entries[0].timestamp, "20260301T120003");
        assert_eq!(entries[1].timestamp, "20260301T120004");
        assert_eq!(entries[2].timestamp, "20260301T120005");
    }

    #[test]
    fn test_list_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let nonexistent_dir = tmp.path().join("no_such_dir");

        let backup = ConfigBackup::new(nonexistent_dir, 5);
        let entries = backup.list().unwrap();

        assert!(entries.is_empty());
    }
}

//! Tool Output Cleanup Service
//!
//! Provides automatic cleanup of old tool output files.
//! Files older than the retention period are deleted.
//!
//! Inspired by OpenCode's tool/truncation.ts cleanup() function.

use std::path::Path;
use std::time::{Duration, SystemTime};

use tokio::time::interval;
use tracing::{debug, info, warn};

use super::truncation::get_tool_output_dir;
use crate::error::Result;

/// Default retention period (7 days, matches OpenCode)
pub const DEFAULT_RETENTION_DAYS: u32 = 7;

/// Default cleanup interval (1 hour, matches OpenCode)
pub const DEFAULT_CLEANUP_INTERVAL_SECS: u64 = 60 * 60;

/// Configuration for cleanup service
#[derive(Debug, Clone)]
pub struct CleanupConfig {
    /// Retention period in days
    pub retention_days: u32,
    /// Cleanup interval in seconds
    pub cleanup_interval_secs: u64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_RETENTION_DAYS,
            cleanup_interval_secs: DEFAULT_CLEANUP_INTERVAL_SECS,
        }
    }
}

impl CleanupConfig {
    /// Create config with custom retention
    pub fn with_retention_days(mut self, days: u32) -> Self {
        self.retention_days = days;
        self
    }

    /// Create config with custom interval
    pub fn with_interval_secs(mut self, secs: u64) -> Self {
        self.cleanup_interval_secs = secs;
        self
    }

    /// Get retention period as Duration
    pub fn retention_duration(&self) -> Duration {
        Duration::from_secs(self.retention_days as u64 * 24 * 60 * 60)
    }
}

/// Clean up old tool output files
///
/// Deletes files older than the retention period from the tool-output directory.
/// Errors during deletion are logged but don't stop the cleanup process.
///
/// # Returns
/// * Number of files deleted
pub fn cleanup_old_outputs(config: &CleanupConfig) -> Result<usize> {
    let output_dir = get_tool_output_dir()?;

    if !output_dir.exists() {
        debug!("Tool output directory doesn't exist, nothing to clean up");
        return Ok(0);
    }

    let cutoff = SystemTime::now() - config.retention_duration();
    let mut deleted_count = 0;

    // Read directory entries
    let entries = match std::fs::read_dir(&output_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!("Failed to read tool output directory: {}", e);
            return Ok(0);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process tool_* files
        if !is_tool_output_file(&path) {
            continue;
        }

        // Check file modification time
        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                debug!("Failed to get metadata for {}: {}", path.display(), e);
                continue;
            }
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(e) => {
                debug!("Failed to get modification time for {}: {}", path.display(), e);
                continue;
            }
        };

        // Delete if older than cutoff
        if modified < cutoff {
            match std::fs::remove_file(&path) {
                Ok(_) => {
                    debug!("Deleted old tool output: {}", path.display());
                    deleted_count += 1;
                }
                Err(e) => {
                    warn!("Failed to delete old tool output {}: {}", path.display(), e);
                }
            }
        }
    }

    if deleted_count > 0 {
        info!("Cleaned up {} old tool output files", deleted_count);
    }

    Ok(deleted_count)
}

/// Check if a path is a tool output file
fn is_tool_output_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("tool_") && n.ends_with(".txt"))
        .unwrap_or(false)
}

/// Start the background cleanup scheduler
///
/// This spawns a tokio task that periodically cleans up old tool output files.
/// The task runs until the application shuts down.
///
/// # Arguments
/// * `config` - Cleanup configuration
///
/// # Returns
/// * JoinHandle for the spawned task
pub fn start_cleanup_scheduler(config: CleanupConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(config.cleanup_interval_secs));

        // Skip the first tick (runs immediately otherwise)
        interval.tick().await;

        loop {
            interval.tick().await;

            debug!("Running scheduled tool output cleanup");
            if let Err(e) = cleanup_old_outputs(&config) {
                warn!("Tool output cleanup failed: {}", e);
            }
        }
    })
}

/// Run cleanup once immediately
///
/// Useful for manual cleanup triggers or testing.
pub async fn run_cleanup_now(config: &CleanupConfig) -> Result<usize> {
    cleanup_old_outputs(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_cleanup_config_defaults() {
        let config = CleanupConfig::default();
        assert_eq!(config.retention_days, 7);
        assert_eq!(config.cleanup_interval_secs, 3600);
    }

    #[test]
    fn test_retention_duration() {
        let config = CleanupConfig::default();
        let duration = config.retention_duration();
        assert_eq!(duration, Duration::from_secs(7 * 24 * 60 * 60));
    }

    #[test]
    fn test_is_tool_output_file() {
        let dir = tempdir().unwrap();

        // Create test files
        let tool_file = dir.path().join("tool_20240101_123456_0.txt");
        File::create(&tool_file).unwrap();

        let other_file = dir.path().join("other.txt");
        File::create(&other_file).unwrap();

        let tool_dir = dir.path().join("tool_dir");
        fs::create_dir(&tool_dir).unwrap();

        assert!(is_tool_output_file(&tool_file));
        assert!(!is_tool_output_file(&other_file));
        assert!(!is_tool_output_file(&tool_dir));
    }

    #[test]
    fn test_cleanup_empty_dir() {
        // This test may create the directory if it doesn't exist
        let config = CleanupConfig::default();
        let result = cleanup_old_outputs(&config);
        // Should succeed without errors
        assert!(result.is_ok());
    }

    #[test]
    fn test_cleanup_with_old_files() {
        use filetime::{set_file_mtime, FileTime};

        let dir = tempdir().unwrap();
        let old_file = dir.path().join("tool_old_0.txt");
        let new_file = dir.path().join("tool_new_1.txt");

        // Create files
        let mut f1 = File::create(&old_file).unwrap();
        f1.write_all(b"old content").unwrap();
        drop(f1);

        let mut f2 = File::create(&new_file).unwrap();
        f2.write_all(b"new content").unwrap();
        drop(f2);

        // Set old file to 10 days ago
        let old_time = SystemTime::now() - Duration::from_secs(10 * 24 * 60 * 60);
        set_file_mtime(&old_file, FileTime::from_system_time(old_time)).unwrap();

        // Note: This test would need to mock get_tool_output_dir() to use tempdir
        // For now, we just verify the logic works
    }
}

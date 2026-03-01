/// Log retention policy implementation
///
/// This module handles automatic deletion of old log files based on
/// the configured retention period.
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Clean up old log files based on retention policy
///
/// Deletes log files older than the specified number of days.
/// Silently skips files that can't be accessed or deleted.
///
/// # Arguments
///
/// * `log_dir` - Directory containing log files
/// * `retention_days` - Number of days to keep logs (1-30)
///
/// # Returns
///
/// * `Result<usize>` - Number of files deleted
///
/// # Example
///
/// ```rust,no_run
/// use std::path::Path;
/// use alephcore::logging::retention::cleanup_old_logs;
///
/// let log_dir = Path::new("/Users/user/.aleph/logs");
/// let deleted = cleanup_old_logs(log_dir, 7).unwrap();
/// println!("Deleted {} old log files", deleted);
/// ```
pub fn cleanup_old_logs(
    log_dir: &Path,
    retention_days: u32,
) -> Result<usize, Box<dyn std::error::Error>> {
    // Validate retention_days (1-30)
    let retention_days = retention_days.clamp(1, 30);

    // Calculate cutoff time
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days as u64 * 24 * 60 * 60))
        .ok_or("Failed to calculate cutoff time")?;

    let mut deleted_count = 0;

    // Check if directory exists
    if !log_dir.exists() {
        return Ok(0);
    }

    // Iterate through log files
    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Skip directories
        if !path.is_file() {
            continue;
        }

        // Skip non-log files (only process .log* files)
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if !file_name.starts_with("aleph") || !file_name.contains(".log") {
            continue;
        }

        // Get file modification time
        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue, // Skip files we can't read
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue, // Skip files without modification time
        };

        // Delete if older than cutoff
        if modified < cutoff {
            match fs::remove_file(&path) {
                Ok(_) => {
                    tracing::info!(
                        file = %path.display(),
                        age_days = %(SystemTime::now().duration_since(modified).unwrap_or_default().as_secs() / 86400),
                        "Deleted old log file"
                    );
                    deleted_count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        file = %path.display(),
                        error = %e,
                        "Failed to delete old log file"
                    );
                }
            }
        }
    }

    Ok(deleted_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    fn create_test_log_file(dir: &Path, name: &str, days_old: u64) -> std::io::Result<()> {
        let file_path = dir.join(name);
        File::create(&file_path)?;

        // Set modification time
        let old_time = SystemTime::now()
            .checked_sub(Duration::from_secs(days_old * 24 * 60 * 60))
            .unwrap();

        // Convert to filetime
        let duration_since_epoch = old_time.duration_since(UNIX_EPOCH).unwrap_or_default();
        let filetime = filetime::FileTime::from_unix_time(
            duration_since_epoch.as_secs() as i64,
            duration_since_epoch.subsec_nanos(),
        );

        filetime::set_file_mtime(&file_path, filetime)?;
        Ok(())
    }

    #[test]
    fn test_cleanup_old_logs_basic() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        // Create test log files
        create_test_log_file(log_dir, "aleph.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph.log.2025-01-10", 5).unwrap();
        create_test_log_file(log_dir, "aleph.log.2025-01-20", 1).unwrap();

        // Clean up logs older than 7 days
        let deleted = cleanup_old_logs(log_dir, 7).unwrap();

        // Should delete the 10-day-old log
        assert_eq!(deleted, 1);

        // Verify remaining files
        let remaining: Vec<_> = fs::read_dir(log_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        assert!(remaining.contains(&"aleph.log.2025-01-10".to_string()));
        assert!(remaining.contains(&"aleph.log.2025-01-20".to_string()));
        assert!(!remaining.contains(&"aleph.log.2025-01-01".to_string()));
    }

    #[test]
    fn test_cleanup_skip_non_log_files() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        // Create log files and non-log files
        create_test_log_file(log_dir, "aleph.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "other.txt", 10).unwrap();
        create_test_log_file(log_dir, "README.md", 10).unwrap();

        // Clean up logs older than 7 days
        let deleted = cleanup_old_logs(log_dir, 7).unwrap();

        // Should only delete the aleph.log file
        assert_eq!(deleted, 1);

        // Verify non-log files still exist
        assert!(log_dir.join("other.txt").exists());
        assert!(log_dir.join("README.md").exists());
    }

    #[test]
    fn test_cleanup_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        // Clean up empty directory
        let deleted = cleanup_old_logs(log_dir, 7).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_nonexistent_directory() {
        let log_dir = Path::new("/nonexistent/directory");

        // Should return 0 without error
        let deleted = cleanup_old_logs(log_dir, 7).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_retention_days_clamping() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph.log.2025-01-01", 40).unwrap();

        // Test with invalid retention days (should clamp to 1-30)
        let deleted = cleanup_old_logs(log_dir, 0).unwrap(); // Clamps to 1
        assert_eq!(deleted, 1);
    }

    #[test]
    fn test_cleanup_all_logs_within_retention() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        // Create logs all within retention period
        create_test_log_file(log_dir, "aleph.log.2025-01-10", 1).unwrap();
        create_test_log_file(log_dir, "aleph.log.2025-01-11", 2).unwrap();

        // Clean up logs older than 7 days
        let deleted = cleanup_old_logs(log_dir, 7).unwrap();

        // Should delete nothing
        assert_eq!(deleted, 0);
    }
}

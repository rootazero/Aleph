/// Log retention policy implementation
///
/// Handles automatic deletion of old log files based on retention period.
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Clean up old log files based on retention policy
///
/// When `component_prefix` is provided, only files matching that prefix are cleaned.
/// When `None`, all `aleph*.log*` files are cleaned.
///
/// # Arguments
///
/// * `log_dir` - Directory containing log files
/// * `retention_days` - Number of days to keep logs (1-30)
/// * `component_prefix` - Optional prefix filter (e.g., "aleph-server")
pub fn cleanup_old_logs(
    log_dir: &Path,
    retention_days: u32,
    component_prefix: Option<&str>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let retention_days = retention_days.clamp(1, 30);

    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days as u64 * 24 * 60 * 60))
        .ok_or("Failed to calculate cutoff time")?;

    let mut deleted_count = 0;

    if !log_dir.exists() {
        return Ok(0);
    }

    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        let prefix = component_prefix.unwrap_or("aleph");
        if !file_name.starts_with(prefix) || !file_name.contains(".log") {
            continue;
        }

        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };

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

        let old_time = SystemTime::now()
            .checked_sub(Duration::from_secs(days_old * 24 * 60 * 60))
            .unwrap();

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

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-server.log.2025-01-10", 5).unwrap();
        create_test_log_file(log_dir, "aleph-server.log.2025-01-20", 1).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, Some("aleph-server")).unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn test_cleanup_component_specific() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-tauri.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-cli.log.2025-01-01", 10).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, Some("aleph-server")).unwrap();
        assert_eq!(deleted, 1);

        assert!(log_dir.join("aleph-tauri.log.2025-01-01").exists());
        assert!(log_dir.join("aleph-cli.log.2025-01-01").exists());
    }

    #[test]
    fn test_cleanup_nonexistent_directory() {
        let log_dir = Path::new("/nonexistent/directory");
        let deleted = cleanup_old_logs(log_dir, 7, None).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let deleted = cleanup_old_logs(temp_dir.path(), 7, None).unwrap();
        assert_eq!(deleted, 0);
    }
}

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
        .checked_sub(Duration::from_secs(
            u64::from(retention_days) * 24 * 60 * 60,
        ))
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

        let is_log_file = if let Some(prefix) = component_prefix {
            if !file_name.starts_with(prefix) {
                continue;
            }
            file_name == format!("{prefix}.log")
                || (file_name.starts_with(&format!("{prefix}.log.")) && {
                    let suffix = &file_name[format!("{prefix}.log.").len()..];
                    suffix.len() == 10
                        && suffix.chars().nth(4) == Some('-')
                        && suffix.chars().nth(7) == Some('-')
                })
        } else {
            // When no prefix is specified, match all aleph-*.log and aleph-*.log.YYYY-MM-DD files
            if !file_name.starts_with("aleph-") {
                continue;
            }
            file_name.to_ascii_lowercase().ends_with(".log")
                || file_name.to_ascii_lowercase().rsplit_once(".log.").is_some_and(|(_, suffix)| {
                    suffix.len() == 10
                        && suffix.chars().nth(4) == Some('-')
                        && suffix.chars().nth(7) == Some('-')
                })
        };
        if !is_log_file {
            continue;
        }

        let Ok(metadata) = fs::metadata(&path) else { continue; };
        let Ok(modified) = metadata.modified() else { continue; };

        if modified < cutoff {
            match fs::remove_file(&path) {
                Ok(()) => {
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
            i64::try_from(duration_since_epoch.as_secs()).unwrap_or(i64::MAX),
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
        create_test_log_file(log_dir, "aleph-desktop.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-cli.log.2025-01-01", 10).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, Some("aleph-server")).unwrap();
        assert_eq!(deleted, 1);

        assert!(log_dir.join("aleph-desktop.log.2025-01-01").exists());
        assert!(log_dir.join("aleph-cli.log.2025-01-01").exists());
    }

    #[test]
    fn test_cleanup_skips_backup_files() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-server.log.backup", 10).unwrap();
        create_test_log_file(log_dir, "aleph.log.backup", 10).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, Some("aleph-server")).unwrap();
        assert_eq!(deleted, 1);

        // Backup files should NOT be deleted
        assert!(log_dir.join("aleph-server.log.backup").exists());
        assert!(log_dir.join("aleph.log.backup").exists());
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

    #[test]
    fn test_cleanup_all_components_with_none_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "aleph-desktop.log.2025-01-10", 5).unwrap();
        create_test_log_file(log_dir, "aleph-cli.log.2025-01-20", 1).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, None).unwrap();
        assert_eq!(deleted, 1);

        assert!(log_dir.join("aleph-desktop.log.2025-01-10").exists());
        assert!(log_dir.join("aleph-cli.log.2025-01-20").exists());
        assert!(!log_dir.join("aleph-server.log.2025-01-01").exists());
    }

    #[test]
    fn test_cleanup_skip_non_log_files_with_none_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 10).unwrap();
        create_test_log_file(log_dir, "other.txt", 10).unwrap();
        create_test_log_file(log_dir, "README.md", 10).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, None).unwrap();
        assert_eq!(deleted, 1);

        assert!(log_dir.join("other.txt").exists());
        assert!(log_dir.join("README.md").exists());
    }

    #[test]
    fn test_cleanup_retention_days_zero_with_none_prefix() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 40).unwrap();
        let deleted = cleanup_old_logs(log_dir, 0, None).unwrap();
        assert_eq!(deleted, 1);
    }
}

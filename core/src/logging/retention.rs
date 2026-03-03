/// Log retention policy — delegates to `aleph-logging` crate
///
/// This module re-exports the cleanup function from `aleph-logging`.
use std::path::Path;

/// Clean up old log files based on retention policy.
///
/// Delegates to `aleph_logging::cleanup_old_logs`.
/// See that function for full documentation.
pub fn cleanup_old_logs(
    log_dir: &Path,
    retention_days: u32,
    component_prefix: Option<&str>,
) -> Result<usize, Box<dyn std::error::Error>> {
    aleph_logging::cleanup_old_logs(log_dir, retention_days, component_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
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

        let remaining: Vec<_> = fs::read_dir(log_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();

        assert!(remaining.contains(&"aleph-server.log.2025-01-10".to_string()));
        assert!(remaining.contains(&"aleph-server.log.2025-01-20".to_string()));
        assert!(!remaining.contains(&"aleph-server.log.2025-01-01".to_string()));
    }

    #[test]
    fn test_cleanup_skip_non_log_files() {
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
    fn test_cleanup_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let deleted = cleanup_old_logs(temp_dir.path(), 7, None).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_nonexistent_directory() {
        let log_dir = Path::new("/nonexistent/directory");
        let deleted = cleanup_old_logs(log_dir, 7, None).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_retention_days_clamping() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-01", 40).unwrap();
        let deleted = cleanup_old_logs(log_dir, 0, None).unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn test_cleanup_all_logs_within_retention() {
        let temp_dir = TempDir::new().unwrap();
        let log_dir = temp_dir.path();

        create_test_log_file(log_dir, "aleph-server.log.2025-01-10", 1).unwrap();
        create_test_log_file(log_dir, "aleph-server.log.2025-01-11", 2).unwrap();

        let deleted = cleanup_old_logs(log_dir, 7, None).unwrap();
        assert_eq!(deleted, 0);
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
}

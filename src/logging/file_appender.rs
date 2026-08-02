/// Log file appender helpers — delegates to `aleph-logging` crate
use std::path::PathBuf;

use crate::logging::LoggingError;

/// Get the log directory path: `~/.aleph/logs/`
pub fn get_log_directory() -> Result<PathBuf, LoggingError> {
    aleph_logging::get_log_directory().map_err(|e| LoggingError::LogDirectory(e.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_directory() {
        // ALEPH_HOME is process-global; lock so other tests don't point it at a
        // temp directory without "aleph" in the path while we read it.
        let _guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("ALEPH_HOME");
        let tmp = std::env::temp_dir().join(".aleph").join("log_dir_test");
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ALEPH_HOME", &tmp);

        let log_dir = get_log_directory().unwrap();
        assert!(log_dir.to_string_lossy().contains("aleph"));
        assert!(log_dir.to_string_lossy().contains("logs"));

        match prev {
            Some(v) => std::env::set_var("ALEPH_HOME", v),
            None => std::env::remove_var("ALEPH_HOME"),
        }
    }

    #[test]
    fn test_log_directory_creation() {
        // Use a temp directory to avoid deleting the real ~/.aleph/logs/
        let temp_dir = tempfile::TempDir::new().unwrap();
        let log_dir = temp_dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        assert!(log_dir.exists());
        assert!(log_dir.is_dir());
        // temp_dir is automatically cleaned up on drop
    }
}

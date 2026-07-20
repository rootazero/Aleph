/// Log file appender — delegates to `aleph-logging` crate
///
/// This module re-exports the shared logging API from `aleph-logging`
/// and provides backward-compatible convenience wrappers for the server.
use std::path::PathBuf;

use crate::logging::LoggingError;

const DEFAULT_RETENTION_DAYS: u32 = 7;
const DEFAULT_FILTER: &str = "info";

/// Initialize file + console logging for a named component.
///
/// Delegates to `aleph_logging::init_component_logging`.
/// See that function for full documentation.
pub fn init_component_logging(
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<(), LoggingError> {
    aleph_logging::init_component_logging(component, retention_days, default_filter)
        .map_err(LoggingError::Init)
}

/// Initialize logging with file appender and PII scrubbing (server defaults).
///
/// Convenience wrapper that calls `init_component_logging("server", 7, "info")`.
pub fn init_file_logging() -> Result<(), LoggingError> {
    init_component_logging("server", DEFAULT_RETENTION_DAYS, DEFAULT_FILTER)
}

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

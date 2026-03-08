/// Log file appender — delegates to `aleph-logging` crate
///
/// This module re-exports the shared logging API from `aleph-logging`
/// and provides backward-compatible convenience wrappers for the server.
use std::path::PathBuf;

/// Initialize file + console logging for a named component.
///
/// Delegates to `aleph_logging::init_component_logging`.
/// See that function for full documentation.
pub fn init_component_logging(
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    aleph_logging::init_component_logging(component, retention_days, default_filter)
}

/// Initialize logging with file appender and PII scrubbing (server defaults).
///
/// Convenience wrapper that calls `init_component_logging("server", 7, "info")`.
pub fn init_file_logging() -> Result<(), Box<dyn std::error::Error>> {
    init_component_logging("server", 7, "info")
}

/// Initialize logging with custom retention policy (server defaults).
pub fn init_file_logging_with_retention(
    retention_days: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    init_component_logging("server", retention_days, "info")
}

/// Get the log directory path: `~/.aleph/logs/`
pub fn get_log_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    aleph_logging::get_log_directory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_directory() {
        let log_dir = get_log_directory().unwrap();
        assert!(log_dir.to_string_lossy().contains("aleph"));
        assert!(log_dir.to_string_lossy().contains("logs"));
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

/// Log file appender with rotation and PII scrubbing
///
/// This module sets up file-based logging with daily rotation and automatic
/// PII scrubbing for privacy protection.
use crate::utils::paths::get_config_dir;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Global initialization guard
static INIT: Once = Once::new();

/// Guard to keep the non-blocking writer alive (using OnceLock for sound interior mutability)
static GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// Initialize logging with file appender and PII scrubbing
///
/// This function sets up:
/// - Console output with PII scrubbing
/// - File output with daily rotation and PII scrubbing
/// - Log directory: `~/.aleph/logs/`
/// - Log files: `aleph-YYYY-MM-DD.log`
/// - Environment-based filtering (RUST_LOG)
/// - Automatic cleanup of old log files
///
/// # Example
///
/// ```rust,no_run
/// use alephcore::logging::init_file_logging;
///
/// init_file_logging().expect("Failed to initialize logging");
/// ```
///
/// # Environment Variables
///
/// - `RUST_LOG`: Controls log level (e.g., "debug", "info", "aleph=debug")
///
/// # Panics
///
/// This function will panic if called more than once (enforced by `Once` guard).
pub fn init_file_logging() -> Result<(), Box<dyn std::error::Error>> {
    init_file_logging_with_retention(7) // Default 7 days retention
}

/// Initialize logging with custom retention policy
///
/// Same as `init_file_logging()` but allows specifying a custom retention period.
///
/// # Arguments
///
/// * `retention_days` - Number of days to keep log files (1-30)
pub fn init_file_logging_with_retention(
    retention_days: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut result = Ok(());

    INIT.call_once(|| {
        match setup_logging(retention_days) {
            Ok(guard) => {
                // Store the guard to keep the non-blocking writer alive
                let _ = GUARD.set(guard);
            }
            Err(e) => {
                result = Err(e);
            }
        }
    });

    result
}

/// Internal function to set up logging infrastructure
fn setup_logging(
    retention_days: u32,
) -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>> {
    // Get log directory: ~/.aleph/logs/ (unified across all platforms)
    let log_dir = get_log_directory()?;

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir)?;

    // Create rolling file appender (daily rotation)
    // Creates files like: aleph.log.2026-01-01
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "aleph.log");

    // Create non-blocking writer for async logging
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    // Set up environment filter
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Create console layer with PII scrubbing
    let console_layer = fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::logging::pii_filter::PiiScrubbingFormat);

    // Create file layer with PII scrubbing
    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::logging::pii_filter::PiiScrubbingFormat);

    // Initialize subscriber with both console and file output
    // Use try_init to avoid panic if already initialized (e.g., by Tauri)
    if tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .is_err()
    {
        // Already initialized, that's fine
        return Err("Tracing already initialized".into());
    }

    // Write initial log entry
    tracing::info!("Logging system initialized");

    // Clean up old log files after logging is initialized
    match crate::logging::cleanup_old_logs(&log_dir, retention_days) {
        Ok(count) if count > 0 => {
            tracing::info!(deleted = count, retention_days, "Cleaned up old log files");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to cleanup old logs");
        }
        _ => {} // No files to delete
    }

    Ok(guard)
}

/// Get the log directory path
///
/// Returns `~/.aleph/logs/` on all platforms (unified path).
/// Uses the same base directory as other Aleph data for consistency.
pub fn get_log_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config_dir = get_config_dir().map_err(|e| e.to_string())?;

    Ok(config_dir.join("logs"))
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
        let log_dir = get_log_directory().unwrap();

        // Clean up if exists
        let _ = std::fs::remove_dir_all(&log_dir);

        // Create directory
        std::fs::create_dir_all(&log_dir).unwrap();

        // Verify it exists
        assert!(log_dir.exists());
        assert!(log_dir.is_dir());

        // Clean up
        let _ = std::fs::remove_dir_all(&log_dir);
    }
}

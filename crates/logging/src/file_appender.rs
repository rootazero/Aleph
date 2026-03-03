/// Log file appender with rotation and PII scrubbing
///
/// Sets up file-based logging with daily rotation and automatic PII scrubbing.
/// Log files are written to `~/.aleph/logs/aleph-{component}.log.YYYY-MM-DD`.
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Global initialization guard
static INIT: Once = Once::new();

/// Guard to keep the non-blocking writer alive
static GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// Initialize file + console logging for a named component.
///
/// - Log file: `~/.aleph/logs/aleph-{component}.log.YYYY-MM-DD`
/// - Daily rotation via tracing-appender
/// - PII scrubbing on both console and file output
/// - Automatic cleanup of files older than `retention_days`
/// - `RUST_LOG` environment variable overrides `default_filter`
///
/// # Arguments
///
/// * `component` - Component name (e.g., "server", "tauri", "cli")
/// * `retention_days` - Number of days to keep log files (1-30)
/// * `default_filter` - Default log filter when `RUST_LOG` is not set
///
/// # Example
///
/// ```rust,no_run
/// use aleph_logging::init_component_logging;
///
/// init_component_logging("server", 7, "info").expect("Failed to init logging");
/// ```
pub fn init_component_logging(
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let component = component.to_owned();
    let default_filter = default_filter.to_owned();
    let mut result = Ok(());

    INIT.call_once(|| {
        match setup_logging(&component, retention_days, &default_filter) {
            Ok(guard) => {
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
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>> {
    let log_dir = get_log_directory()?;

    std::fs::create_dir_all(&log_dir)?;

    // Creates files like: aleph-server.log.2026-03-03
    let file_prefix = format!("aleph-{}.log", component);
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, &file_prefix);

    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    // RUST_LOG overrides default_filter
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));

    let console_layer = fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::pii_filter::PiiScrubbingFormat);

    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::pii_filter::PiiScrubbingFormat);

    if tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .is_err()
    {
        return Err("Tracing already initialized".into());
    }

    tracing::info!(component, "Logging system initialized");

    // Clean up old log files for this component
    let component_prefix = format!("aleph-{}", component);
    match crate::retention::cleanup_old_logs(&log_dir, retention_days, Some(&component_prefix)) {
        Ok(count) if count > 0 => {
            tracing::info!(deleted = count, retention_days, component, "Cleaned up old log files");
        }
        Err(e) => {
            tracing::warn!(error = %e, component, "Failed to cleanup old logs");
        }
        _ => {}
    }

    Ok(guard)
}

/// Get the log directory path: `~/.aleph/logs/`
pub fn get_log_directory() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    Ok(home.join(".aleph").join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_log_directory() {
        let log_dir = get_log_directory().unwrap();
        assert!(log_dir.to_string_lossy().contains(".aleph"));
        assert!(log_dir.to_string_lossy().contains("logs"));
    }
}

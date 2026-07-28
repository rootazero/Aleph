use std::io::IsTerminal;
/// Log file appender with rotation and PII scrubbing
///
/// Sets up file-based logging with daily rotation and automatic PII scrubbing.
/// Log files are written to `~/.aleph/logs/aleph-{component}.log.YYYY-MM-DD`.
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter};

/// Guard to keep the non-blocking writer alive, plus initialization result
static GUARD: OnceLock<Result<tracing_appender::non_blocking::WorkerGuard, String>> =
    OnceLock::new();
static FILTER_RELOAD: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> =
    OnceLock::new();

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
/// * `component` - Component name (e.g., "server", "desktop", "cli")
/// * `retention_days` - Number of days to keep log files (1-30)
/// * `default_filter` - Default log filter when `RUST_LOG` is not set
///
/// # Notes
///
/// This function uses a global `OnceLock` and can only successfully initialize
/// logging once per process. Subsequent calls will return `Ok(())` but ignore
/// the provided `component`, `retention_days`, and `default_filter` arguments.
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
    let result = GUARD.get_or_init(|| setup_logging(component, retention_days, default_filter));

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e.clone().into()),
    }
}

pub fn set_log_level(filter: &str) -> Result<(), String> {
    let handle = FILTER_RELOAD
        .get()
        .ok_or_else(|| "Logging has not been initialized".to_string())?;
    handle
        .modify(|current| {
            *current = EnvFilter::new(filter);
        })
        .map_err(|e| format!("Failed to reload log filter: {e}"))
}

/// Internal function to set up logging infrastructure
fn setup_logging(
    component: &str,
    retention_days: u32,
    default_filter: &str,
) -> Result<tracing_appender::non_blocking::WorkerGuard, String> {
    let log_dir = get_log_directory()?;

    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

    // Creates files like: aleph-server.log.2026-03-03
    let file_prefix = format!("aleph-{component}.log");
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, &file_prefix);

    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    // RUST_LOG overrides default_filter
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let (filter_layer, filter_handle) = reload::Layer::new(env_filter);

    // Console layer only when stdout is an interactive terminal. In daemon
    // mode `daemonize()` redirects stdout to gateway.log BEFORE this runs, so
    // is_terminal() is false there — dropping the console layer stops
    // gateway.log from accumulating a duplicate of the rotating file_layer.
    // `Option<L>` implements `Layer`, so `None` contributes nothing to the
    // subscriber and the registry chain below is unchanged.
    let console_layer = std::io::stdout().is_terminal().then(|| {
        fmt::layer()
            .with_target(true)
            .with_level(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .event_format(crate::pii_filter::PiiScrubbingFormat)
    });

    let file_layer = fmt::layer()
        .with_writer(non_blocking_file)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::pii_filter::PiiScrubbingFormat);

    if tracing_subscriber::registry()
        .with(filter_layer)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .is_err()
    {
        return Err("Tracing already initialized".into());
    }

    if FILTER_RELOAD.set(filter_handle).is_err() {
        return Err("Logging filter was already initialized".into());
    }

    tracing::info!(component, "Logging system initialized");

    // Clean up old log files for this component
    let component_prefix = format!("aleph-{component}");
    match crate::retention::cleanup_old_logs(&log_dir, retention_days, Some(&component_prefix)) {
        Ok(count) if count > 0 => {
            tracing::info!(
                deleted = count,
                retention_days,
                component,
                "Cleaned up old log files"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, component, "Failed to cleanup old logs");
        }
        _ => {}
    }

    Ok(guard)
}

/// Get the log directory path: `<.aleph>/logs/`.
///
/// Resolution priority (mirrors `alephcore::utils::paths::get_config_dir`,
/// re-implemented here because this crate must stay free of an alephcore
/// dependency):
/// 1. `ALEPH_HOME` — points directly at the `.aleph` data directory.
/// 2. `$HOME` — the Unix standard; honoured here so logs follow the same
///    relocation as config/data (`dirs::home_dir()` ignores `$HOME` on macOS,
///    which split an isolated test server's logs into the real ~/.aleph).
/// 3. `dirs::home_dir()` — last-resort platform lookup.
pub fn get_log_directory() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("ALEPH_HOME") {
        return Ok(PathBuf::from(dir).join("logs"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .ok_or("Cannot determine home directory")?;
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

/// Logging infrastructure
///
/// This module provides the log directory path used by the server and runtime
/// control over the global log level. File/console logging initialization and
/// PII scrubbing live in the shared `aleph-logging` crate.
pub mod error;
pub mod file_appender;
pub mod level_control;

pub use error::LoggingError;
pub use file_appender::get_log_directory;
pub use level_control::{get_log_level, set_log_level, LogLevel};

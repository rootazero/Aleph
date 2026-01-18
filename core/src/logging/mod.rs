/// Logging infrastructure with privacy protection
///
/// This module provides structured logging using the `tracing` crate with
/// PII scrubbing to ensure no sensitive information is written to log files.
pub mod file_appender;
pub mod level_control;
pub mod pii_filter;
pub mod retention;

pub use file_appender::{get_log_directory, init_file_logging};
pub use level_control::{get_log_level, set_log_level, LogLevel};
pub use pii_filter::{create_pii_scrubbing_layer, PiiScrubbingLayer};
pub use retention::cleanup_old_logs;

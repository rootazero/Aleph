//! Logging context for BDD tests

use std::path::PathBuf;
use tempfile::TempDir;

/// Context for logging system tests
#[derive(Default)]
pub struct LoggingContext {
    /// Temporary directory for test isolation
    pub temp_dir: Option<TempDir>,
    /// Log directory path
    pub log_dir: Option<PathBuf>,
    /// Old log files created for testing
    pub old_log_files: Vec<PathBuf>,
    /// Recent log files created for testing
    pub recent_log_files: Vec<PathBuf>,
    /// Non-log files created for testing
    pub non_log_files: Vec<PathBuf>,
    /// Cleanup result (number of files deleted)
    pub cleanup_result: Option<Result<usize, String>>,
    /// Log directory result from get_log_directory
    pub log_directory_result: Option<PathBuf>,
    /// Thread results for concurrent testing
    pub thread_results: Vec<Result<String, String>>,
    /// PII layer active flag
    pub pii_layer_active: bool,
}

impl std::fmt::Debug for LoggingContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggingContext")
            .field("temp_dir", &self.temp_dir.as_ref().map(|_| "TempDir"))
            .field("log_dir", &self.log_dir)
            .field("old_log_files", &self.old_log_files.len())
            .field("recent_log_files", &self.recent_log_files.len())
            .field("non_log_files", &self.non_log_files.len())
            .field("cleanup_result", &self.cleanup_result)
            .field("log_directory_result", &self.log_directory_result)
            .field("thread_results", &self.thread_results.len())
            .field("pii_layer_active", &self.pii_layer_active)
            .finish()
    }
}

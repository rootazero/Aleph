//! Global state management for file operations
//!
//! Manages working directory and written files tracking across sessions.

use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing::info;

/// Global working directory for the current session/topic
/// This is set at the start of processing and used for relative path resolution
/// Using global Mutex instead of thread-local to work across async task boundaries
static CURRENT_WORKING_DIR: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

/// Global registry of files written during the current session
/// This allows tracking generated files for attachment display across threads
static WRITTEN_FILES: Lazy<Mutex<Vec<WrittenFile>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Information about a file written during tool execution
#[derive(Debug, Clone)]
pub struct WrittenFile {
    /// Full path to the written file
    pub path: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Operation that created this file (write, copy, etc.)
    pub operation: String,
}

/// Set the working directory for the current session
/// Relative paths will be resolved to this directory
pub fn set_working_dir(dir: Option<PathBuf>) {
    if let Ok(mut wd) = CURRENT_WORKING_DIR.lock() {
        *wd = dir;
    }
}

/// Get the working directory for the current session
pub fn get_working_dir() -> Option<PathBuf> {
    CURRENT_WORKING_DIR.lock().ok().and_then(|wd| wd.clone())
}

/// Clear the written files registry for a new session
pub fn clear_written_files() {
    if let Ok(mut files) = WRITTEN_FILES.lock() {
        files.clear();
        info!("Cleared written files registry");
    }
}

/// Record a file that was written during tool execution
pub fn record_written_file(path: PathBuf, size: u64, operation: &str) {
    if let Ok(mut files) = WRITTEN_FILES.lock() {
        info!(
            path = %path.display(),
            size = size,
            operation = operation,
            current_count = files.len(),
            "Recording written file to global registry"
        );
        files.push(WrittenFile {
            path,
            size,
            operation: operation.to_string(),
        });
    }
}

/// Get all files written during the current session and clear the registry
pub fn take_written_files() -> Vec<WrittenFile> {
    if let Ok(mut files) = WRITTEN_FILES.lock() {
        let result = std::mem::take(&mut *files);
        info!(file_count = result.len(), "Taking written files from global registry");
        result
    } else {
        Vec::new()
    }
}

/// Get all files written during the current session without clearing
pub fn get_written_files() -> Vec<WrittenFile> {
    WRITTEN_FILES.lock().ok().map(|f| f.clone()).unwrap_or_default()
}

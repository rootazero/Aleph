//! Global state management for file operations
//!
//! Manages working directory and written files tracking across sessions.

use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use tracing::{info, warn};
use walkdir::WalkDir;

/// Global working directory for the current session/topic
/// This is set at the start of processing and used for relative path resolution
/// Using global Mutex instead of thread-local to work across async task boundaries
static CURRENT_WORKING_DIR: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

/// Global registry of files written during the current session
/// This allows tracking generated files for attachment display across threads
static WRITTEN_FILES: Lazy<Mutex<Vec<WrittenFile>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Timestamp marking when the current session started
/// Used to detect files created during the session by comparing modification times
static SESSION_START_TIME: Lazy<Mutex<Option<SystemTime>>> = Lazy::new(|| Mutex::new(None));

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

/// Mark the start of a new session for file tracking
/// This records the current time as the baseline for detecting newly created files
pub fn mark_session_start() {
    if let Ok(mut start_time) = SESSION_START_TIME.lock() {
        let now = SystemTime::now();
        *start_time = Some(now);
        info!("Marked session start time for file tracking");
    }
}

/// Scan the working directory for files created/modified after session start
/// Returns files that were created during the current session but not explicitly tracked
pub fn scan_new_files_in_working_dir() -> Vec<WrittenFile> {
    let working_dir = match get_working_dir() {
        Some(dir) => dir,
        None => {
            info!("No working directory set, skipping file scan");
            return Vec::new();
        }
    };

    let start_time = match SESSION_START_TIME.lock().ok().and_then(|t| *t) {
        Some(time) => time,
        None => {
            info!("No session start time set, skipping file scan");
            return Vec::new();
        }
    };

    if !working_dir.exists() {
        info!(dir = %working_dir.display(), "Working directory does not exist, skipping scan");
        return Vec::new();
    }

    info!(
        dir = %working_dir.display(),
        "Scanning working directory for new files created during session"
    );

    // Get already tracked files to avoid duplicates
    let tracked_paths: std::collections::HashSet<PathBuf> = get_written_files()
        .iter()
        .map(|f| f.path.clone())
        .collect();

    let mut new_files = Vec::new();

    // Walk the working directory
    for entry in WalkDir::new(&working_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Skip directories
        if !path.is_file() {
            continue;
        }

        // Skip files that are already tracked
        if tracked_paths.contains(path) {
            continue;
        }

        // Check if file was created/modified after session start
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified_time) = metadata.modified() {
                if modified_time >= start_time {
                    let size = metadata.len();
                    info!(
                        path = %path.display(),
                        size = size,
                        "Found new file created during session"
                    );
                    new_files.push(WrittenFile {
                        path: path.to_path_buf(),
                        size,
                        operation: "session_scan".to_string(),
                    });
                }
            } else {
                warn!(path = %path.display(), "Failed to get file modification time");
            }
        }
    }

    info!(
        new_file_count = new_files.len(),
        tracked_count = tracked_paths.len(),
        "Completed working directory scan"
    );

    new_files
}

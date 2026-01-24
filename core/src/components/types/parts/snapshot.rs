//! Filesystem snapshot types for session revert capability

use serde::{Deserialize, Serialize};

/// Individual file snapshot entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    /// File path (relative or absolute)
    pub path: String,
    /// Content hash (SHA256 or similar)
    pub hash: String,
}

impl FileSnapshot {
    /// Create a new file snapshot entry
    pub fn new(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            hash: hash.into(),
        }
    }
}

/// Filesystem snapshot - captures file state at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPart {
    /// Unique snapshot identifier
    pub snapshot_id: String,
    /// List of files with their hashes
    pub files: Vec<FileSnapshot>,
    /// When the snapshot was taken
    pub timestamp: i64,
}

impl SnapshotPart {
    /// Create a new empty snapshot
    pub fn new(snapshot_id: impl Into<String>) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            files: Vec::new(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create with files
    pub fn with_files(snapshot_id: impl Into<String>, files: Vec<FileSnapshot>) -> Self {
        Self {
            snapshot_id: snapshot_id.into(),
            files,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Add a file to the snapshot
    pub fn add_file(&mut self, path: impl Into<String>, hash: impl Into<String>) {
        self.files.push(FileSnapshot::new(path, hash));
    }
}

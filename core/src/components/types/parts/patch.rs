//! File change types for patches between snapshots

use serde::{Deserialize, Serialize};

/// Type of file change
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileChangeType {
    /// File was added
    Added,
    /// File was modified
    Modified,
    /// File was deleted
    Deleted,
}

/// Individual file change record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path
    pub path: String,
    /// Type of change
    pub change_type: FileChangeType,
    /// New content hash (for Added/Modified), None for Deleted
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl FileChange {
    /// Create a new file added change
    pub fn added(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Added,
            content_hash: Some(hash.into()),
        }
    }

    /// Create a new file modified change
    pub fn modified(path: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Modified,
            content_hash: Some(hash.into()),
        }
    }

    /// Create a new file deleted change
    pub fn deleted(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            change_type: FileChangeType::Deleted,
            content_hash: None,
        }
    }
}

/// Patch part - records file changes between snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPart {
    /// Unique patch identifier
    pub patch_id: String,
    /// Base snapshot this patch applies to
    pub base_snapshot_id: String,
    /// List of file changes
    pub changes: Vec<FileChange>,
}

impl PatchPart {
    /// Create a new empty patch
    pub fn new(patch_id: impl Into<String>, base_snapshot_id: impl Into<String>) -> Self {
        Self {
            patch_id: patch_id.into(),
            base_snapshot_id: base_snapshot_id.into(),
            changes: Vec::new(),
        }
    }

    /// Create with changes
    pub fn with_changes(
        patch_id: impl Into<String>,
        base_snapshot_id: impl Into<String>,
        changes: Vec<FileChange>,
    ) -> Self {
        Self {
            patch_id: patch_id.into(),
            base_snapshot_id: base_snapshot_id.into(),
            changes,
        }
    }

    /// Add a change to the patch
    pub fn add_change(&mut self, change: FileChange) {
        self.changes.push(change);
    }
}

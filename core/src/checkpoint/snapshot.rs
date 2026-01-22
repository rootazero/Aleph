//! Checkpoint and file snapshot definitions

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Unique identifier for a checkpoint
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub String);

impl CheckpointId {
    /// Create a new checkpoint ID with timestamp and random suffix
    pub fn new() -> Self {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let suffix: u32 = rand::random::<u32>() % 10000;
        Self(format!("ckpt_{}_{:04}", timestamp, suffix))
    }

    /// Create from existing string
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Snapshot of a single file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    /// Absolute path to the file
    pub path: PathBuf,

    /// File content at snapshot time (None if file didn't exist)
    pub content: Option<Vec<u8>>,

    /// File size in bytes
    pub size: usize,

    /// Whether the file existed before the snapshot
    pub existed: bool,

    /// SHA-256 hash of content for integrity verification
    pub content_hash: String,
}

impl FileSnapshot {
    /// Create a snapshot of an existing file
    pub fn from_file(path: PathBuf, content: Vec<u8>) -> Self {
        let hash = Self::compute_hash(&content);
        Self {
            path,
            size: content.len(),
            content: Some(content),
            existed: true,
            content_hash: hash,
        }
    }

    /// Create a snapshot indicating file didn't exist
    pub fn non_existent(path: PathBuf) -> Self {
        Self {
            path,
            content: None,
            size: 0,
            existed: false,
            content_hash: String::new(),
        }
    }

    /// Compute SHA-256 hash of content
    fn compute_hash(content: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content);
        format!("{:x}", hasher.finalize())
    }

    /// Verify content integrity
    pub fn verify_integrity(&self) -> bool {
        match &self.content {
            Some(content) => Self::compute_hash(content) == self.content_hash,
            None => self.content_hash.is_empty(),
        }
    }
}

/// A checkpoint containing multiple file snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier
    pub id: CheckpointId,

    /// Session ID this checkpoint belongs to
    pub session_id: String,

    /// Human-readable label
    pub label: Option<String>,

    /// When the checkpoint was created
    pub created_at: DateTime<Utc>,

    /// File snapshots (path -> snapshot)
    pub snapshots: HashMap<PathBuf, FileSnapshot>,

    /// Description of what action triggered this checkpoint
    pub trigger_action: Option<String>,

    /// Whether this checkpoint has been rolled back
    pub rolled_back: bool,
}

impl Checkpoint {
    /// Create a new checkpoint
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            id: CheckpointId::new(),
            session_id: session_id.into(),
            label: None,
            created_at: Utc::now(),
            snapshots: HashMap::new(),
            trigger_action: None,
            rolled_back: false,
        }
    }

    /// Add a file snapshot
    pub fn add_snapshot(&mut self, snapshot: FileSnapshot) {
        self.snapshots.insert(snapshot.path.clone(), snapshot);
    }

    /// Get snapshot for a specific file
    pub fn get_snapshot(&self, path: &PathBuf) -> Option<&FileSnapshot> {
        self.snapshots.get(path)
    }

    /// Set the trigger action description
    pub fn with_trigger(mut self, action: impl Into<String>) -> Self {
        self.trigger_action = Some(action.into());
        self
    }

    /// Set a human-readable label
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Get number of files in this checkpoint
    pub fn file_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Get total size of all snapshots
    pub fn total_size(&self) -> usize {
        self.snapshots.values().map(|s| s.size).sum()
    }

    /// Verify integrity of all snapshots
    pub fn verify_all(&self) -> bool {
        self.snapshots.values().all(|s| s.verify_integrity())
    }

    /// Mark as rolled back
    pub fn mark_rolled_back(&mut self) {
        self.rolled_back = true;
    }
}

/// Summary information about a checkpoint (without full content)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummary {
    /// Checkpoint ID
    pub id: CheckpointId,

    /// Session ID
    pub session_id: String,

    /// Human-readable label
    pub label: Option<String>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Number of files
    pub file_count: usize,

    /// Total size in bytes
    pub total_size: usize,

    /// Trigger action description
    pub trigger_action: Option<String>,

    /// Files that were snapshotted (paths only)
    pub files: Vec<PathBuf>,

    /// Whether rolled back
    pub rolled_back: bool,
}

impl From<&Checkpoint> for CheckpointSummary {
    fn from(cp: &Checkpoint) -> Self {
        Self {
            id: cp.id.clone(),
            session_id: cp.session_id.clone(),
            label: cp.label.clone(),
            created_at: cp.created_at,
            file_count: cp.file_count(),
            total_size: cp.total_size(),
            trigger_action: cp.trigger_action.clone(),
            files: cp.snapshots.keys().cloned().collect(),
            rolled_back: cp.rolled_back,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_id_generation() {
        let id1 = CheckpointId::new();
        let id2 = CheckpointId::new();
        assert_ne!(id1, id2);
        assert!(id1.as_str().starts_with("ckpt_"));
    }

    #[test]
    fn test_file_snapshot_from_file() {
        let content = b"hello world".to_vec();
        let snapshot = FileSnapshot::from_file(PathBuf::from("/test/file.txt"), content.clone());

        assert!(snapshot.existed);
        assert_eq!(snapshot.size, 11);
        assert!(snapshot.content.is_some());
        assert!(snapshot.verify_integrity());
    }

    #[test]
    fn test_file_snapshot_non_existent() {
        let snapshot = FileSnapshot::non_existent(PathBuf::from("/test/missing.txt"));

        assert!(!snapshot.existed);
        assert_eq!(snapshot.size, 0);
        assert!(snapshot.content.is_none());
        assert!(snapshot.verify_integrity());
    }

    #[test]
    fn test_checkpoint_creation() {
        let mut checkpoint = Checkpoint::new("session_123");
        checkpoint.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/test/a.txt"),
            b"content a".to_vec(),
        ));
        checkpoint.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/test/b.txt"),
            b"content b".to_vec(),
        ));

        assert_eq!(checkpoint.file_count(), 2);
        assert_eq!(checkpoint.total_size(), 18);
        assert!(checkpoint.verify_all());
    }

    #[test]
    fn test_checkpoint_summary() {
        let mut checkpoint = Checkpoint::new("session_456")
            .with_label("Before edit")
            .with_trigger("Edit file.rs");

        checkpoint.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/test/file.rs"),
            b"fn main() {}".to_vec(),
        ));

        let summary = CheckpointSummary::from(&checkpoint);
        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.label.as_deref(), Some("Before edit"));
        assert_eq!(summary.trigger_action.as_deref(), Some("Edit file.rs"));
    }
}

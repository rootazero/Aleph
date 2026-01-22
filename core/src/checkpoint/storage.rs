//! Checkpoint storage backends

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use async_trait::async_trait;

use super::snapshot::{Checkpoint, CheckpointId, CheckpointSummary};
use crate::error::Result;

/// Storage backend for checkpoints
#[async_trait]
pub trait CheckpointStorage: Send + Sync {
    /// Store a checkpoint
    async fn store(&self, checkpoint: Checkpoint) -> Result<()>;

    /// Load a checkpoint by ID
    async fn load(&self, id: &CheckpointId) -> Result<Option<Checkpoint>>;

    /// Delete a checkpoint by ID
    async fn delete(&self, id: &CheckpointId) -> Result<bool>;

    /// List all checkpoints for a session
    async fn list_for_session(&self, session_id: &str) -> Result<Vec<CheckpointSummary>>;

    /// List all checkpoints (across all sessions)
    async fn list_all(&self) -> Result<Vec<CheckpointSummary>>;

    /// Get storage statistics
    async fn stats(&self) -> Result<StorageStats>;

    /// Cleanup old checkpoints (keep only the most recent N per session)
    async fn cleanup(&self, keep_per_session: usize) -> Result<usize>;
}

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    /// Total number of checkpoints
    pub total_checkpoints: usize,
    /// Total storage size in bytes
    pub total_size: usize,
    /// Number of unique sessions
    pub session_count: usize,
}

/// In-memory storage for checkpoints (primarily for testing)
pub struct MemoryStorage {
    checkpoints: RwLock<HashMap<CheckpointId, Checkpoint>>,
    max_checkpoints: usize,
}

impl MemoryStorage {
    /// Create a new in-memory storage
    pub fn new() -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
            max_checkpoints: 100,
        }
    }

    /// Create with custom maximum checkpoints
    pub fn with_max(max: usize) -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
            max_checkpoints: max,
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckpointStorage for MemoryStorage {
    async fn store(&self, checkpoint: Checkpoint) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().unwrap();

        // Cleanup if at capacity
        if checkpoints.len() >= self.max_checkpoints {
            // Remove oldest checkpoint
            if let Some(oldest_id) = checkpoints
                .values()
                .min_by_key(|c| c.created_at)
                .map(|c| c.id.clone())
            {
                checkpoints.remove(&oldest_id);
            }
        }

        checkpoints.insert(checkpoint.id.clone(), checkpoint);
        Ok(())
    }

    async fn load(&self, id: &CheckpointId) -> Result<Option<Checkpoint>> {
        let checkpoints = self.checkpoints.read().unwrap();
        Ok(checkpoints.get(id).cloned())
    }

    async fn delete(&self, id: &CheckpointId) -> Result<bool> {
        let mut checkpoints = self.checkpoints.write().unwrap();
        Ok(checkpoints.remove(id).is_some())
    }

    async fn list_for_session(&self, session_id: &str) -> Result<Vec<CheckpointSummary>> {
        let checkpoints = self.checkpoints.read().unwrap();
        let mut summaries: Vec<CheckpointSummary> = checkpoints
            .values()
            .filter(|c| c.session_id == session_id)
            .map(CheckpointSummary::from)
            .collect();

        // Sort by creation time (newest first)
        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    async fn list_all(&self) -> Result<Vec<CheckpointSummary>> {
        let checkpoints = self.checkpoints.read().unwrap();
        let mut summaries: Vec<CheckpointSummary> =
            checkpoints.values().map(CheckpointSummary::from).collect();

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    async fn stats(&self) -> Result<StorageStats> {
        let checkpoints = self.checkpoints.read().unwrap();
        let sessions: std::collections::HashSet<_> =
            checkpoints.values().map(|c| &c.session_id).collect();

        Ok(StorageStats {
            total_checkpoints: checkpoints.len(),
            total_size: checkpoints.values().map(|c| c.total_size()).sum(),
            session_count: sessions.len(),
        })
    }

    async fn cleanup(&self, keep_per_session: usize) -> Result<usize> {
        let mut checkpoints = self.checkpoints.write().unwrap();

        // Group by session
        let mut by_session: HashMap<String, Vec<&CheckpointId>> = HashMap::new();
        for (id, checkpoint) in checkpoints.iter() {
            by_session
                .entry(checkpoint.session_id.clone())
                .or_default()
                .push(id);
        }

        // Find checkpoints to remove
        let mut to_remove = Vec::new();
        for (_, mut ids) in by_session {
            if ids.len() > keep_per_session {
                // Sort by creation time and keep newest
                ids.sort_by(|a, b| {
                    let ca = checkpoints.get(*a).unwrap();
                    let cb = checkpoints.get(*b).unwrap();
                    cb.created_at.cmp(&ca.created_at)
                });

                // Remove older ones
                for id in ids.into_iter().skip(keep_per_session) {
                    to_remove.push(id.clone());
                }
            }
        }

        let removed_count = to_remove.len();
        for id in to_remove {
            checkpoints.remove(&id);
        }

        Ok(removed_count)
    }
}

/// File-based storage for checkpoints (persistent)
pub struct FileStorage {
    base_dir: PathBuf,
    max_checkpoints_per_session: usize,
}

impl FileStorage {
    /// Create a new file-based storage
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        // Create base directory if it doesn't exist
        std::fs::create_dir_all(&base_dir)?;

        Ok(Self {
            base_dir,
            max_checkpoints_per_session: 50,
        })
    }

    /// Get path for a checkpoint file
    fn checkpoint_path(&self, id: &CheckpointId) -> PathBuf {
        self.base_dir.join(format!("{}.json", id.as_str()))
    }

    /// Get path for the index file
    fn index_path(&self) -> PathBuf {
        self.base_dir.join("checkpoints_index.json")
    }
}

#[async_trait]
impl CheckpointStorage for FileStorage {
    async fn store(&self, checkpoint: Checkpoint) -> Result<()> {
        let path = self.checkpoint_path(&checkpoint.id);

        // Serialize checkpoint
        let json = serde_json::to_string_pretty(&checkpoint)?;

        // Write to file
        tokio::fs::write(&path, json).await?;

        Ok(())
    }

    async fn load(&self, id: &CheckpointId) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let checkpoint: Checkpoint = serde_json::from_str(&content)?;
        Ok(Some(checkpoint))
    }

    async fn delete(&self, id: &CheckpointId) -> Result<bool> {
        let path = self.checkpoint_path(id);

        if path.exists() {
            tokio::fs::remove_file(&path).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn list_for_session(&self, session_id: &str) -> Result<Vec<CheckpointSummary>> {
        let all = self.list_all().await?;
        Ok(all
            .into_iter()
            .filter(|s| s.session_id == session_id)
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<CheckpointSummary>> {
        let mut summaries = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "json") {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(&content) {
                        summaries.push(CheckpointSummary::from(&checkpoint));
                    }
                }
            }
        }

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    async fn stats(&self) -> Result<StorageStats> {
        let summaries = self.list_all().await?;
        let sessions: std::collections::HashSet<_> =
            summaries.iter().map(|s| &s.session_id).collect();

        Ok(StorageStats {
            total_checkpoints: summaries.len(),
            total_size: summaries.iter().map(|s| s.total_size).sum(),
            session_count: sessions.len(),
        })
    }

    async fn cleanup(&self, keep_per_session: usize) -> Result<usize> {
        let summaries = self.list_all().await?;

        // Group by session
        let mut by_session: HashMap<String, Vec<&CheckpointSummary>> = HashMap::new();
        for summary in &summaries {
            by_session
                .entry(summary.session_id.clone())
                .or_default()
                .push(summary);
        }

        let mut removed = 0;
        for (_, mut session_checkpoints) in by_session {
            if session_checkpoints.len() > keep_per_session {
                // Sort by creation time (newest first)
                session_checkpoints.sort_by(|a, b| b.created_at.cmp(&a.created_at));

                // Remove older ones
                for summary in session_checkpoints.into_iter().skip(keep_per_session) {
                    if self.delete(&summary.id).await? {
                        removed += 1;
                    }
                }
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::checkpoint::snapshot::FileSnapshot;

    #[tokio::test]
    async fn test_memory_storage_basic() {
        let storage = MemoryStorage::new();

        // Create and store a checkpoint
        let mut checkpoint = Checkpoint::new("session_1");
        checkpoint.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/test/file.txt"),
            b"content".to_vec(),
        ));

        let id = checkpoint.id.clone();
        storage.store(checkpoint).await.unwrap();

        // Load it back
        let loaded = storage.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().file_count(), 1);
    }

    #[tokio::test]
    async fn test_memory_storage_list() {
        let storage = MemoryStorage::new();

        // Store checkpoints for different sessions
        let mut cp1 = Checkpoint::new("session_1");
        cp1.add_snapshot(FileSnapshot::non_existent(PathBuf::from("/a.txt")));

        let mut cp2 = Checkpoint::new("session_1");
        cp2.add_snapshot(FileSnapshot::non_existent(PathBuf::from("/b.txt")));

        let mut cp3 = Checkpoint::new("session_2");
        cp3.add_snapshot(FileSnapshot::non_existent(PathBuf::from("/c.txt")));

        storage.store(cp1).await.unwrap();
        storage.store(cp2).await.unwrap();
        storage.store(cp3).await.unwrap();

        // List for session_1
        let list = storage.list_for_session("session_1").await.unwrap();
        assert_eq!(list.len(), 2);

        // List all
        let all = storage.list_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_memory_storage_cleanup() {
        let storage = MemoryStorage::new();

        // Store 5 checkpoints for same session
        for i in 0..5 {
            let mut cp = Checkpoint::new("session_1");
            cp.add_snapshot(FileSnapshot::non_existent(PathBuf::from(format!("/{}.txt", i))));
            storage.store(cp).await.unwrap();
            // Small delay to ensure different timestamps
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Cleanup keeping only 2
        let removed = storage.cleanup(2).await.unwrap();
        assert_eq!(removed, 3);

        let remaining = storage.list_for_session("session_1").await.unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_storage_stats() {
        let storage = MemoryStorage::new();

        let mut cp1 = Checkpoint::new("session_1");
        cp1.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/a.txt"),
            b"12345".to_vec(),
        ));

        let mut cp2 = Checkpoint::new("session_2");
        cp2.add_snapshot(FileSnapshot::from_file(
            PathBuf::from("/b.txt"),
            b"67890".to_vec(),
        ));

        storage.store(cp1).await.unwrap();
        storage.store(cp2).await.unwrap();

        let stats = storage.stats().await.unwrap();
        assert_eq!(stats.total_checkpoints, 2);
        assert_eq!(stats.total_size, 10);
        assert_eq!(stats.session_count, 2);
    }
}

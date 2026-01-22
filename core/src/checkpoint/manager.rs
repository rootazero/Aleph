//! Checkpoint manager for coordinating snapshots and rollbacks

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::snapshot::{Checkpoint, CheckpointId, CheckpointSummary, FileSnapshot};
use super::storage::{CheckpointStorage, FileStorage, MemoryStorage};
use crate::error::{AetherError, Result};

/// Configuration for the checkpoint manager
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Storage directory for checkpoints
    pub storage_dir: Option<PathBuf>,

    /// Maximum checkpoints to keep per session
    pub max_per_session: usize,

    /// Maximum total checkpoints
    pub max_total: usize,

    /// Whether to use file-based storage (vs in-memory)
    pub persistent: bool,

    /// Auto-cleanup interval (in number of checkpoints created)
    pub cleanup_interval: usize,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            storage_dir: None,
            max_per_session: 50,
            max_total: 500,
            persistent: true,
            cleanup_interval: 10,
        }
    }
}

impl CheckpointConfig {
    /// Create config for in-memory storage (testing)
    pub fn memory() -> Self {
        Self {
            persistent: false,
            ..Default::default()
        }
    }

    /// Create config with custom storage directory
    pub fn with_storage_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            storage_dir: Some(dir.into()),
            persistent: true,
            ..Default::default()
        }
    }
}

/// Result of a rollback operation
#[derive(Debug, Clone)]
pub struct RollbackResult {
    /// Checkpoint that was rolled back to
    pub checkpoint_id: CheckpointId,

    /// Files that were restored
    pub restored_files: Vec<PathBuf>,

    /// Files that were deleted (didn't exist in checkpoint)
    pub deleted_files: Vec<PathBuf>,

    /// Files that failed to restore
    pub failed_files: Vec<(PathBuf, String)>,
}

impl RollbackResult {
    /// Check if rollback was fully successful
    pub fn is_success(&self) -> bool {
        self.failed_files.is_empty()
    }

    /// Get total number of files affected
    pub fn total_affected(&self) -> usize {
        self.restored_files.len() + self.deleted_files.len()
    }
}

/// Checkpoint manager for file snapshots and rollback
pub struct CheckpointManager {
    storage: Arc<dyn CheckpointStorage>,
    config: CheckpointConfig,
    current_session_id: RwLock<Option<String>>,
    checkpoint_count: RwLock<usize>,
}

impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new(config: CheckpointConfig) -> Result<Self> {
        let storage: Arc<dyn CheckpointStorage> = if config.persistent {
            let dir = config.storage_dir.clone().unwrap_or_else(|| {
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("aether")
                    .join("checkpoints")
            });
            Arc::new(FileStorage::new(dir)?)
        } else {
            Arc::new(MemoryStorage::with_max(config.max_total))
        };

        Ok(Self {
            storage,
            config,
            current_session_id: RwLock::new(None),
            checkpoint_count: RwLock::new(0),
        })
    }

    /// Create with custom storage backend
    pub fn with_storage(storage: Arc<dyn CheckpointStorage>, config: CheckpointConfig) -> Self {
        Self {
            storage,
            config,
            current_session_id: RwLock::new(None),
            checkpoint_count: RwLock::new(0),
        }
    }

    /// Set the current session ID
    pub async fn set_session(&self, session_id: impl Into<String>) {
        let mut current = self.current_session_id.write().await;
        *current = Some(session_id.into());
    }

    /// Get the current session ID
    pub async fn current_session(&self) -> Option<String> {
        self.current_session_id.read().await.clone()
    }

    /// Create a checkpoint before editing files
    ///
    /// This snapshots the current state of the specified files so they
    /// can be restored later if needed.
    pub async fn create_checkpoint<P: AsRef<Path>>(
        &self,
        files: &[P],
        trigger_action: Option<&str>,
    ) -> Result<CheckpointId> {
        let session_id = self
            .current_session_id
            .read()
            .await
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let mut checkpoint = Checkpoint::new(&session_id);

        if let Some(action) = trigger_action {
            checkpoint = checkpoint.with_trigger(action);
        }

        // Snapshot each file
        for path in files {
            let path = path.as_ref();
            let abs_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()?.join(path)
            };

            let snapshot = self.snapshot_file(&abs_path).await?;
            checkpoint.add_snapshot(snapshot);
        }

        let id = checkpoint.id.clone();
        info!(
            "Created checkpoint {} with {} files",
            id,
            checkpoint.file_count()
        );

        // Store the checkpoint
        self.storage.store(checkpoint).await?;

        // Maybe run cleanup
        let mut count = self.checkpoint_count.write().await;
        *count += 1;
        if *count % self.config.cleanup_interval == 0 {
            drop(count); // Release lock before cleanup
            self.cleanup().await?;
        }

        Ok(id)
    }

    /// Create a checkpoint with automatic file discovery
    ///
    /// Useful when you want to snapshot files that will be modified
    /// without knowing exactly which ones beforehand.
    pub async fn checkpoint_before_edit(
        &self,
        file: impl AsRef<Path>,
        action: &str,
    ) -> Result<CheckpointId> {
        self.create_checkpoint(&[file.as_ref()], Some(action)).await
    }

    /// Snapshot a single file
    async fn snapshot_file(&self, path: &Path) -> Result<FileSnapshot> {
        if path.exists() {
            match tokio::fs::read(path).await {
                Ok(content) => {
                    debug!("Snapshotted file: {:?} ({} bytes)", path, content.len());
                    Ok(FileSnapshot::from_file(path.to_path_buf(), content))
                }
                Err(e) => {
                    warn!("Failed to read file {:?}: {}", path, e);
                    Err(AetherError::IoError(e.to_string()))
                }
            }
        } else {
            debug!("File does not exist (will be created): {:?}", path);
            Ok(FileSnapshot::non_existent(path.to_path_buf()))
        }
    }

    /// Rollback to a specific checkpoint
    ///
    /// Restores all files in the checkpoint to their snapshotted state.
    /// Files that didn't exist in the checkpoint will be deleted.
    pub async fn rollback(&self, checkpoint_id: &CheckpointId) -> Result<RollbackResult> {
        let checkpoint = self
            .storage
            .load(checkpoint_id)
            .await?
            .ok_or_else(|| AetherError::NotFound(format!("Checkpoint {} not found", checkpoint_id)))?;

        info!(
            "Rolling back to checkpoint {} ({} files)",
            checkpoint_id,
            checkpoint.file_count()
        );

        let mut result = RollbackResult {
            checkpoint_id: checkpoint_id.clone(),
            restored_files: Vec::new(),
            deleted_files: Vec::new(),
            failed_files: Vec::new(),
        };

        for (path, snapshot) in &checkpoint.snapshots {
            match self.restore_file(path, snapshot).await {
                Ok(RestoreAction::Restored) => {
                    result.restored_files.push(path.clone());
                }
                Ok(RestoreAction::Deleted) => {
                    result.deleted_files.push(path.clone());
                }
                Err(e) => {
                    result.failed_files.push((path.clone(), e.to_string()));
                }
            }
        }

        // Mark checkpoint as rolled back
        if let Some(mut cp) = self.storage.load(checkpoint_id).await? {
            cp.mark_rolled_back();
            self.storage.store(cp).await?;
        }

        if result.is_success() {
            info!(
                "Rollback complete: {} restored, {} deleted",
                result.restored_files.len(),
                result.deleted_files.len()
            );
        } else {
            warn!(
                "Rollback partial: {} failed out of {}",
                result.failed_files.len(),
                checkpoint.file_count()
            );
        }

        Ok(result)
    }

    /// Restore a single file from snapshot
    async fn restore_file(&self, path: &Path, snapshot: &FileSnapshot) -> Result<RestoreAction> {
        if snapshot.existed {
            // File existed - restore content
            if let Some(content) = &snapshot.content {
                // Ensure parent directory exists
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }

                tokio::fs::write(path, content).await?;
                debug!("Restored file: {:?}", path);
                Ok(RestoreAction::Restored)
            } else {
                Err(AetherError::CorruptData(format!(
                    "Snapshot for {:?} has no content",
                    path
                )))
            }
        } else {
            // File didn't exist - delete if it exists now
            if path.exists() {
                tokio::fs::remove_file(path).await?;
                debug!("Deleted file (didn't exist before): {:?}", path);
                Ok(RestoreAction::Deleted)
            } else {
                // File still doesn't exist, nothing to do
                Ok(RestoreAction::Deleted)
            }
        }
    }

    /// List checkpoints for the current session
    pub async fn list_checkpoints(&self) -> Result<Vec<CheckpointSummary>> {
        let session_id = self
            .current_session_id
            .read()
            .await
            .clone()
            .unwrap_or_else(|| "default".to_string());

        self.storage.list_for_session(&session_id).await
    }

    /// List all checkpoints
    pub async fn list_all_checkpoints(&self) -> Result<Vec<CheckpointSummary>> {
        self.storage.list_all().await
    }

    /// Get the most recent checkpoint for current session
    pub async fn latest_checkpoint(&self) -> Result<Option<CheckpointSummary>> {
        let list = self.list_checkpoints().await?;
        Ok(list.into_iter().next())
    }

    /// Delete a specific checkpoint
    pub async fn delete_checkpoint(&self, id: &CheckpointId) -> Result<bool> {
        self.storage.delete(id).await
    }

    /// Run cleanup to remove old checkpoints
    pub async fn cleanup(&self) -> Result<usize> {
        info!(
            "Running checkpoint cleanup (keeping {} per session)",
            self.config.max_per_session
        );
        self.storage.cleanup(self.config.max_per_session).await
    }

    /// Get storage statistics
    pub async fn stats(&self) -> Result<super::storage::StorageStats> {
        self.storage.stats().await
    }
}

/// Action taken when restoring a file
enum RestoreAction {
    Restored,
    Deleted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_manager() -> (CheckpointManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = CheckpointConfig {
            storage_dir: Some(temp_dir.path().join("checkpoints")),
            persistent: true,
            max_per_session: 5,
            cleanup_interval: 100, // Don't auto-cleanup during tests
            ..Default::default()
        };
        let manager = CheckpointManager::new(config).unwrap();
        manager.set_session("test_session").await;
        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_create_and_list_checkpoints() {
        let (manager, temp_dir) = create_test_manager().await;

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "original content").await.unwrap();

        // Create checkpoint
        let id = manager
            .create_checkpoint(&[&test_file], Some("test edit"))
            .await
            .unwrap();

        // List checkpoints
        let list = manager.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].trigger_action.as_deref(), Some("test edit"));
    }

    #[tokio::test]
    async fn test_rollback() {
        let (manager, temp_dir) = create_test_manager().await;

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "original content").await.unwrap();

        // Create checkpoint
        let id = manager
            .create_checkpoint(&[&test_file], Some("before edit"))
            .await
            .unwrap();

        // Modify the file
        tokio::fs::write(&test_file, "modified content").await.unwrap();

        // Verify modification
        let content = tokio::fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "modified content");

        // Rollback
        let result = manager.rollback(&id).await.unwrap();
        assert!(result.is_success());
        assert_eq!(result.restored_files.len(), 1);

        // Verify rollback
        let content = tokio::fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "original content");
    }

    #[tokio::test]
    async fn test_rollback_new_file() {
        let (manager, temp_dir) = create_test_manager().await;

        // File doesn't exist yet
        let test_file = temp_dir.path().join("new_file.txt");

        // Create checkpoint (file doesn't exist)
        let id = manager
            .create_checkpoint(&[&test_file], Some("before create"))
            .await
            .unwrap();

        // Create the file
        tokio::fs::write(&test_file, "new content").await.unwrap();
        assert!(test_file.exists());

        // Rollback should delete the file
        let result = manager.rollback(&id).await.unwrap();
        assert!(result.is_success());
        assert_eq!(result.deleted_files.len(), 1);

        // File should be gone
        assert!(!test_file.exists());
    }

    #[tokio::test]
    async fn test_memory_storage_manager() {
        let storage = Arc::new(MemoryStorage::new());
        let config = CheckpointConfig::memory();
        let manager = CheckpointManager::with_storage(storage, config);
        manager.set_session("memory_session").await;

        // Create checkpoint
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "content").await.unwrap();

        let id = manager
            .create_checkpoint(&[&test_file], None)
            .await
            .unwrap();

        // Verify
        let list = manager.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
    }

    #[tokio::test]
    async fn test_latest_checkpoint() {
        let (manager, temp_dir) = create_test_manager().await;

        let test_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&test_file, "content").await.unwrap();

        // Create multiple checkpoints
        manager.create_checkpoint(&[&test_file], Some("first")).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        let id2 = manager.create_checkpoint(&[&test_file], Some("second")).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        let id3 = manager.create_checkpoint(&[&test_file], Some("third")).await.unwrap();

        // Latest should be the most recent
        let latest = manager.latest_checkpoint().await.unwrap().unwrap();
        assert_eq!(latest.id, id3);
        assert_eq!(latest.trigger_action.as_deref(), Some("third"));
    }
}

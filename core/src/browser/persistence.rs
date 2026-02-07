//! Browser State Persistence
//!
//! Handles saving and restoring browser context state for hot recovery.
//! Supports cookies, localStorage, and session metadata persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs;

use super::{BrowserError, BrowserResult};

/// Persisted context metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedContext {
    /// Context ID
    pub context_id: String,

    /// Task ID (for ephemeral contexts)
    pub task_id: Option<String>,

    /// Whether this is the primary context
    pub is_primary: bool,

    /// User data directory path
    pub user_data_dir: Option<PathBuf>,

    /// Last access timestamp
    pub last_accessed: SystemTime,

    /// Creation timestamp
    pub created_at: SystemTime,

    /// Domain locks held by this context
    pub domain_locks: Vec<String>,
}

/// Browser pool state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSnapshot {
    /// Snapshot version
    pub version: u32,

    /// Snapshot timestamp
    pub timestamp: SystemTime,

    /// Primary context metadata
    pub primary_context: Option<PersistedContext>,

    /// Ephemeral contexts metadata
    pub ephemeral_contexts: HashMap<String, PersistedContext>,

    /// Active instance count
    pub active_instances: usize,
}

impl PoolSnapshot {
    /// Create a new empty snapshot
    pub fn new() -> Self {
        Self {
            version: 1,
            timestamp: SystemTime::now(),
            primary_context: None,
            ephemeral_contexts: HashMap::new(),
            active_instances: 0,
        }
    }
}

impl Default for PoolSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistence manager for browser pool state
pub struct PersistenceManager {
    /// Base directory for persistence
    base_dir: PathBuf,

    /// Snapshot file path
    snapshot_path: PathBuf,
}

impl PersistenceManager {
    /// Create a new persistence manager
    pub fn new(base_dir: PathBuf) -> Self {
        let snapshot_path = base_dir.join("pool_snapshot.json");
        Self {
            base_dir,
            snapshot_path,
        }
    }

    /// Initialize persistence directory
    pub async fn init(&self) -> BrowserResult<()> {
        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to create persistence dir: {}", e)))?;
        Ok(())
    }

    /// Save pool snapshot to disk
    pub async fn save_snapshot(&self, snapshot: &PoolSnapshot) -> BrowserResult<()> {
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| BrowserError::Internal(format!("Failed to serialize snapshot: {}", e)))?;

        fs::write(&self.snapshot_path, json)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to write snapshot: {}", e)))?;

        tracing::debug!("Saved pool snapshot to {:?}", self.snapshot_path);
        Ok(())
    }

    /// Load pool snapshot from disk
    pub async fn load_snapshot(&self) -> BrowserResult<Option<PoolSnapshot>> {
        if !self.snapshot_path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&self.snapshot_path)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to read snapshot: {}", e)))?;

        let snapshot: PoolSnapshot = serde_json::from_str(&json)
            .map_err(|e| BrowserError::Internal(format!("Failed to deserialize snapshot: {}", e)))?;

        tracing::debug!("Loaded pool snapshot from {:?}", self.snapshot_path);
        Ok(Some(snapshot))
    }

    /// Delete snapshot file
    pub async fn clear_snapshot(&self) -> BrowserResult<()> {
        if self.snapshot_path.exists() {
            fs::remove_file(&self.snapshot_path)
                .await
                .map_err(|e| BrowserError::Internal(format!("Failed to delete snapshot: {}", e)))?;
            tracing::debug!("Cleared pool snapshot");
        }
        Ok(())
    }

    /// Check if snapshot exists
    pub fn has_snapshot(&self) -> bool {
        self.snapshot_path.exists()
    }

    /// Get snapshot age in seconds
    pub async fn snapshot_age(&self) -> BrowserResult<Option<u64>> {
        if !self.snapshot_path.exists() {
            return Ok(None);
        }

        let metadata = fs::metadata(&self.snapshot_path)
            .await
            .map_err(|e| BrowserError::Internal(format!("Failed to read snapshot metadata: {}", e)))?;

        let modified = metadata.modified()
            .map_err(|e| BrowserError::Internal(format!("Failed to get modification time: {}", e)))?;

        let age = SystemTime::now()
            .duration_since(modified)
            .map_err(|e| BrowserError::Internal(format!("Failed to calculate age: {}", e)))?
            .as_secs();

        Ok(Some(age))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_persistence_manager_creation() {
        let temp_dir = std::env::temp_dir().join("aleph_browser_test");
        let manager = PersistenceManager::new(temp_dir.clone());

        assert!(manager.init().await.is_ok());
        assert!(temp_dir.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_snapshot_save_load() {
        let temp_dir = std::env::temp_dir().join("aleph_browser_test_snapshot");
        let manager = PersistenceManager::new(temp_dir.clone());
        manager.init().await.unwrap();

        let mut snapshot = PoolSnapshot::new();
        snapshot.active_instances = 2;
        snapshot.primary_context = Some(PersistedContext {
            context_id: "primary".to_string(),
            task_id: None,
            is_primary: true,
            user_data_dir: Some(PathBuf::from("/tmp/chrome")),
            last_accessed: SystemTime::now(),
            created_at: SystemTime::now(),
            domain_locks: vec![],
        });

        // Save
        assert!(manager.save_snapshot(&snapshot).await.is_ok());
        assert!(manager.has_snapshot());

        // Load
        let loaded = manager.load_snapshot().await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.active_instances, 2);
        assert!(loaded.primary_context.is_some());

        // Cleanup
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_snapshot_clear() {
        let temp_dir = std::env::temp_dir().join("aleph_browser_test_clear");
        let manager = PersistenceManager::new(temp_dir.clone());
        manager.init().await.unwrap();

        let snapshot = PoolSnapshot::new();
        manager.save_snapshot(&snapshot).await.unwrap();
        assert!(manager.has_snapshot());

        manager.clear_snapshot().await.unwrap();
        assert!(!manager.has_snapshot());

        // Cleanup
        let _ = std::fs::remove_dir_all(temp_dir);
    }
}

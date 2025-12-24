/// Memory cleanup service for retention policy enforcement
///
/// This module provides functionality to automatically delete old memories
/// based on the configured retention policy.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;

use super::database::VectorDatabase;

/// Service for cleaning up old memories based on retention policy
pub struct CleanupService {
    database: Arc<VectorDatabase>,
    retention_days: u32,
}

impl CleanupService {
    /// Create a new cleanup service
    ///
    /// # Arguments
    /// * `db_path` - Path to the database file
    /// * `retention_days` - Number of days to retain memories (0 = never delete)
    ///
    /// # Returns
    /// Result containing the cleanup service or an error
    pub fn new(db_path: PathBuf, retention_days: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let database = Arc::new(VectorDatabase::new(db_path)?);
        Ok(Self {
            database,
            retention_days,
        })
    }

    /// Cleanup old memories that exceed the retention period
    ///
    /// Memories older than `retention_days` will be deleted from the database.
    /// If retention_days is 0, this method does nothing (infinite retention).
    ///
    /// # Returns
    /// Result containing the number of deleted memories or an error
    pub fn cleanup_old_memories(&self) -> Result<u64, Box<dyn std::error::Error>> {
        // If retention_days is 0, never delete anything
        if self.retention_days == 0 {
            println!("[Memory Cleanup] Retention policy set to infinite (0 days), skipping cleanup");
            return Ok(0);
        }

        // Calculate cutoff timestamp (current time - retention_days)
        let cutoff_timestamp = chrono::Utc::now()
            .timestamp()
            - (self.retention_days as i64 * 86400); // 86400 seconds per day

        println!(
            "[Memory Cleanup] Starting cleanup: deleting memories older than {} days (cutoff timestamp: {})",
            self.retention_days,
            cutoff_timestamp
        );

        // Delete old memories
        let deleted_count = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                self.database.delete_older_than(cutoff_timestamp)
            )
        })?;

        println!("[Memory Cleanup] Cleanup complete: deleted {} old memories", deleted_count);
        Ok(deleted_count)
    }

    /// Start background cleanup task that runs daily
    ///
    /// This spawns a Tokio task that periodically runs cleanup.
    /// The task runs immediately on start, then repeats every 24 hours.
    ///
    /// # Returns
    /// A JoinHandle to the background task
    pub fn start_background_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(86400)); // 24 hours

            loop {
                interval.tick().await;

                match self.cleanup_old_memories() {
                    Ok(count) => {
                        println!("[Memory Cleanup] Background cleanup completed: {} memories deleted", count);
                    }
                    Err(e) => {
                        eprintln!("[Memory Cleanup] Background cleanup failed: {}", e);
                    }
                }
            }
        })
    }

    /// Update retention policy
    ///
    /// # Arguments
    /// * `retention_days` - New retention period in days (0 = never delete)
    pub fn update_retention_days(&mut self, retention_days: u32) {
        println!(
            "[Memory Cleanup] Updating retention policy from {} to {} days",
            self.retention_days,
            retention_days
        );
        self.retention_days = retention_days;
    }

    /// Get current retention policy
    ///
    /// # Returns
    /// The current retention period in days (0 = infinite retention)
    pub fn get_retention_days(&self) -> u32 {
        self.retention_days
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cleanup_service_creation() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_cleanup.db");

        let service = CleanupService::new(db_path, 90);
        assert!(service.is_ok());

        let service = service.unwrap();
        assert_eq!(service.get_retention_days(), 90);
    }

    #[test]
    fn test_retention_days_zero_no_deletion() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_cleanup_zero.db");

        let service = CleanupService::new(db_path, 0).unwrap();
        let result = service.cleanup_old_memories();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // No deletions with retention_days = 0
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cleanup_old_memories() {
        use super::super::context::{ContextAnchor, MemoryEntry};

        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_cleanup_old.db");

        // Create database and insert test data
        let db = VectorDatabase::new(db_path.clone()).unwrap();

        // Insert a very old memory (100 days ago)
        let old_timestamp = chrono::Utc::now().timestamp() - (100 * 86400);
        let old_context = ContextAnchor {
            app_bundle_id: "com.test.old".to_string(),
            window_title: "Old Window".to_string(),
            timestamp: old_timestamp,
        };
        let embedding = vec![0.1; 384];
        let old_memory = MemoryEntry::with_embedding(
            "old-id".to_string(),
            old_context,
            "old input".to_string(),
            "old output".to_string(),
            embedding.clone(),
        );
        db.insert_memory(old_memory).await.unwrap();

        // Insert a recent memory (1 day ago)
        let recent_timestamp = chrono::Utc::now().timestamp() - 86400;
        let recent_context = ContextAnchor {
            app_bundle_id: "com.test.recent".to_string(),
            window_title: "Recent Window".to_string(),
            timestamp: recent_timestamp,
        };
        let recent_memory = MemoryEntry::with_embedding(
            "recent-id".to_string(),
            recent_context,
            "recent input".to_string(),
            "recent output".to_string(),
            embedding,
        );
        db.insert_memory(recent_memory).await.unwrap();

        // Create cleanup service with 90-day retention
        let service = CleanupService::new(db_path, 90).unwrap();

        // Run cleanup
        let deleted_count = service.cleanup_old_memories().unwrap();

        // Should delete 1 old memory, keep the recent one
        assert_eq!(deleted_count, 1);

        // Verify the recent memory is still there
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
    }

    #[test]
    fn test_update_retention_policy() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_update_retention.db");

        let mut service = CleanupService::new(db_path, 90).unwrap();
        assert_eq!(service.get_retention_days(), 90);

        service.update_retention_days(30);
        assert_eq!(service.get_retention_days(), 30);

        service.update_retention_days(0);
        assert_eq!(service.get_retention_days(), 0);
    }

    #[tokio::test]
    async fn test_background_task_does_not_crash() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_background.db");

        let service = Arc::new(CleanupService::new(db_path, 90).unwrap());
        let handle = service.start_background_task();

        // Let it run briefly to ensure it doesn't panic
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Abort the task
        handle.abort();

        // Give it time to abort
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

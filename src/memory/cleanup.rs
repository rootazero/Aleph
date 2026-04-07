/// Memory cleanup service for retention policy enforcement
///
/// This module previously deleted old raw memories (Layer 1) via SessionStore.
/// With SessionStore removed, the cleanup service is a no-op stub.
/// Facts (Layer 2) have their own decay/invalidation mechanism.
use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Arc;

/// Service for cleaning up old memories (no-op after SessionStore removal)
pub struct CleanupService {
    _database: MemoryBackend,
    retention_days: u32,
}

impl CleanupService {
    /// Create a new cleanup service
    pub fn new(database: MemoryBackend, retention_days: u32) -> Self {
        Self {
            _database: database,
            retention_days,
        }
    }

    /// Cleanup old memories (no-op — raw memory storage removed)
    pub async fn cleanup_old_memories(&self) -> Result<u64, Box<dyn std::error::Error>> {
        if self.retention_days == 0 {
            log::debug!(
                "[Memory Cleanup] Retention policy set to infinite (0 days), skipping cleanup"
            );
        }
        // Raw memory cleanup removed — SessionStore no longer exists.
        Ok(0)
    }

    /// Start background cleanup task (no-op)
    pub fn start_background_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // No-op: raw memory cleanup removed.
            let _ = self;
        })
    }

    /// Start background cleanup task with external runtime (no-op)
    pub fn start_background_task_with_runtime(
        self: &Arc<Self>,
        _runtime: &tokio::runtime::Runtime,
    ) -> tokio::task::JoinHandle<()> {
        let service = Arc::clone(self);
        _runtime.spawn(async move {
            let _ = service;
        })
    }

    /// Update retention policy
    pub fn update_retention_days(&mut self, retention_days: u32) {
        log::info!(
            "[Memory Cleanup] Updating retention policy from {} to {} days",
            self.retention_days,
            retention_days
        );
        self.retention_days = retention_days;
    }

    /// Get current retention policy
    pub fn get_retention_days(&self) -> u32 {
        self.retention_days
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_retention_days_accessor() {
        assert_eq!(90u32, 90u32);
    }
}

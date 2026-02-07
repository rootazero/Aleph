//! Context Registry
//!
//! Manages browser context lifecycle for the BrowserPool.
//! Implements the Hybrid isolation strategy with Primary and Ephemeral contexts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

#[cfg(feature = "browser")]
use chromiumoxide::Page;

/// Context handle wrapping CDP Page
#[cfg(feature = "browser")]
pub type ContextHandle = Arc<Page>;

#[cfg(not(feature = "browser"))]
pub type ContextHandle = Arc<()>;

/// Context ID type
pub type ContextId = String;

/// Task ID type
pub type TaskId = String;

/// Isolation level for contexts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    /// Shared context (Primary)
    Shared,
    /// Isolated context (Ephemeral)
    Isolated,
}

/// Context metadata
#[derive(Debug, Clone)]
pub struct ContextMetadata {
    pub creation_time: SystemTime,
    pub last_access: SystemTime,
    pub isolation_level: IsolationLevel,
    pub persistent: bool,
    pub user_data_dir: Option<PathBuf>,
}

impl ContextMetadata {
    /// Create new metadata for a context
    pub fn new(isolation_level: IsolationLevel, persistent: bool, user_data_dir: Option<PathBuf>) -> Self {
        let now = SystemTime::now();
        Self {
            creation_time: now,
            last_access: now,
            isolation_level,
            persistent,
            user_data_dir,
        }
    }

    /// Update last access time
    pub fn touch(&mut self) {
        self.last_access = SystemTime::now();
    }
}

/// Context Registry for managing browser contexts
pub struct ContextRegistry {
    /// Primary persistent context (user's digital identity)
    primary_context: Arc<RwLock<Option<ContextHandle>>>,

    /// Ephemeral contexts (task-specific isolation)
    ephemeral_contexts: Arc<RwLock<HashMap<TaskId, ContextHandle>>>,

    /// Domain-based locking (prevent same-domain conflicts)
    domain_locks: Arc<RwLock<HashMap<String, TaskId>>>,

    /// Context metadata
    context_metadata: Arc<RwLock<HashMap<ContextId, ContextMetadata>>>,
}

impl ContextRegistry {
    /// Create a new context registry
    pub fn new() -> Self {
        Self {
            primary_context: Arc::new(RwLock::new(None)),
            ephemeral_contexts: Arc::new(RwLock::new(HashMap::new())),
            domain_locks: Arc::new(RwLock::new(HashMap::new())),
            context_metadata: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set the primary context
    pub async fn set_primary_context(&self, context: ContextHandle, user_data_dir: Option<PathBuf>) {
        let mut primary = self.primary_context.write().await;
        *primary = Some(context);

        // Store metadata
        let metadata = ContextMetadata::new(IsolationLevel::Shared, true, user_data_dir);
        self.context_metadata.write().await.insert("primary".to_string(), metadata);
    }

    /// Get the primary context
    pub async fn get_primary_context(&self) -> Option<ContextHandle> {
        let primary = self.primary_context.read().await;
        primary.clone()
    }

    /// Create an ephemeral context for a task
    pub async fn create_ephemeral_context(&self, task_id: TaskId, context: ContextHandle) {
        let mut ephemeral = self.ephemeral_contexts.write().await;
        ephemeral.insert(task_id.clone(), context);

        // Store metadata
        let metadata = ContextMetadata::new(IsolationLevel::Isolated, false, None);
        self.context_metadata.write().await.insert(task_id, metadata);
    }

    /// Get an ephemeral context by task ID
    pub async fn get_ephemeral_context(&self, task_id: &TaskId) -> Option<ContextHandle> {
        let ephemeral: tokio::sync::RwLockReadGuard<HashMap<TaskId, ContextHandle>> =
            self.ephemeral_contexts.read().await;
        ephemeral.get(task_id).cloned()
    }

    /// Remove an ephemeral context
    pub async fn remove_ephemeral_context(&self, task_id: &TaskId) -> Option<ContextHandle> {
        let mut ephemeral: tokio::sync::RwLockWriteGuard<HashMap<TaskId, ContextHandle>> =
            self.ephemeral_contexts.write().await;
        let context = ephemeral.remove(task_id);

        // Remove metadata
        self.context_metadata.write().await.remove(task_id);

        context
    }

    /// Lock a domain for a task
    pub async fn lock_domain(&self, domain: String, task_id: TaskId) -> Result<(), String> {
        let mut locks = self.domain_locks.write().await;

        if let Some(existing_task) = locks.get(&domain) {
            return Err(format!("Domain {} is locked by task {}", domain, existing_task));
        }

        locks.insert(domain, task_id);
        Ok(())
    }

    /// Unlock a domain
    pub async fn unlock_domain(&self, domain: &str) {
        let mut locks = self.domain_locks.write().await;
        locks.remove(domain);
    }

    /// Check if a domain is locked
    pub async fn is_domain_locked(&self, domain: &str) -> bool {
        let locks = self.domain_locks.read().await;
        locks.contains_key(domain)
    }

    /// Get the task ID that locked a domain
    pub async fn get_domain_lock_owner(&self, domain: &str) -> Option<TaskId> {
        let locks = self.domain_locks.read().await;
        locks.get(domain).cloned()
    }

    /// Get metadata for a context
    pub async fn get_metadata(&self, context_id: &ContextId) -> Option<ContextMetadata> {
        let metadata = self.context_metadata.read().await;
        metadata.get(context_id).cloned()
    }

    /// Update last access time for a context
    pub async fn touch_context(&self, context_id: &ContextId) {
        let mut metadata = self.context_metadata.write().await;
        if let Some(meta) = metadata.get_mut(context_id) {
            meta.touch();
        }
    }

    /// Get all ephemeral context IDs
    pub async fn list_ephemeral_contexts(&self) -> Vec<TaskId> {
        let ephemeral: tokio::sync::RwLockReadGuard<HashMap<TaskId, ContextHandle>> =
            self.ephemeral_contexts.read().await;
        ephemeral.keys().cloned().collect()
    }

    /// Clear all ephemeral contexts
    pub async fn clear_ephemeral_contexts(&self) {
        let mut ephemeral: tokio::sync::RwLockWriteGuard<HashMap<TaskId, ContextHandle>> =
            self.ephemeral_contexts.write().await;
        ephemeral.clear();

        // Clear metadata for ephemeral contexts
        let mut metadata = self.context_metadata.write().await;
        metadata.retain(|id, _| id == "primary");
    }
}

impl Default for ContextRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_registry_creation() {
        let registry = ContextRegistry::new();
        assert!(registry.get_primary_context().await.is_none());
    }

    #[tokio::test]
    async fn test_domain_locking() {
        let registry = ContextRegistry::new();

        // Lock domain
        let result = registry.lock_domain("example.com".to_string(), "task-1".to_string()).await;
        assert!(result.is_ok());

        // Try to lock again
        let result = registry.lock_domain("example.com".to_string(), "task-2".to_string()).await;
        assert!(result.is_err());

        // Check lock owner
        let owner = registry.get_domain_lock_owner("example.com").await;
        assert_eq!(owner, Some("task-1".to_string()));

        // Unlock
        registry.unlock_domain("example.com").await;
        assert!(!registry.is_domain_locked("example.com").await);
    }

    #[tokio::test]
    async fn test_ephemeral_context_lifecycle() {
        let registry = ContextRegistry::new();

        // List should be empty
        assert_eq!(registry.list_ephemeral_contexts().await.len(), 0);

        // Clear should not panic
        registry.clear_ephemeral_contexts().await;
    }
}

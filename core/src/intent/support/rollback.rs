//! Rollback Support for Undo Capabilities
//!
//! This module provides rollback capabilities for reversible tool operations,
//! enabling undo functionality during task execution.
//!
//! # Architecture
//!
//! ```text
//! Tool Execution
//!      ↓ (completes successfully)
//! ┌─────────────────────────────────────────┐
//! │           RollbackManager               │
//! │                                         │
//! │  1. Tool provides RollbackEntry         │
//! │  2. Manager stores entry with config    │
//! │  3. On rollback request:                │
//! │     - Execute in reverse order (LIFO)   │
//! │     - Use registered handlers           │
//! │     - Return aggregated results         │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use crate::intent::rollback::{RollbackCapable, RollbackResult, RollbackManager};
//!
//! #[async_trait]
//! impl RollbackCapable for FileCreateHandler {
//!     async fn rollback(&self, rollback_data: &Value) -> RollbackResult {
//!         if let Some(path) = rollback_data.get("path").and_then(|v| v.as_str()) {
//!             std::fs::remove_file(path)?;
//!             RollbackResult::success(format!("Deleted file: {}", path))
//!         } else {
//!             RollbackResult::failure("Missing path in rollback data")
//!         }
//!     }
//!
//!     fn rollback_description(&self) -> &str {
//!         "Deletes the created file"
//!     }
//! }
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// =============================================================================
// Rollback Result
// =============================================================================

/// Result of a rollback operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResult {
    /// Whether the rollback succeeded
    pub success: bool,

    /// Human-readable message describing what was rolled back
    pub message: String,

    /// Error details if rollback failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RollbackResult {
    /// Create a successful rollback result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            error: None,
        }
    }

    /// Create a failed rollback result
    pub fn failure(error: impl Into<String>) -> Self {
        let err = error.into();
        Self {
            success: false,
            message: format!("Rollback failed: {}", err),
            error: Some(err),
        }
    }

    /// Create a skipped rollback result
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            success: true,
            message: format!("Skipped: {}", reason.into()),
            error: None,
        }
    }
}

// =============================================================================
// Rollback Capable Trait
// =============================================================================

/// Trait for tools that support rollback operations
///
/// Implement this trait for tools that can undo their operations.
/// The rollback data is collected during execution and passed to
/// the rollback method when undo is requested.
#[async_trait]
pub trait RollbackCapable: Send + Sync {
    /// Execute rollback using saved data
    ///
    /// # Arguments
    ///
    /// * `data` - Data collected during the original execution
    ///
    /// # Returns
    ///
    /// `RollbackResult` indicating success or failure
    async fn rollback(&self, data: &Value) -> RollbackResult;

    /// Description of what the rollback does
    ///
    /// Used for logging and UI display.
    fn rollback_description(&self) -> &str;
}

// =============================================================================
// Rollback Entry
// =============================================================================

/// Entry for a rollback operation
///
/// Stores information about a completed step that can be rolled back.
#[derive(Debug, Clone)]
pub struct RollbackEntry {
    /// Unique identifier for this step
    pub step_id: String,

    /// Name of the tool that was executed
    pub tool_name: String,

    /// Data needed for rollback (e.g., file path, original content)
    pub rollback_data: Value,

    /// When this entry was created
    pub created_at: Instant,
}

impl RollbackEntry {
    /// Create a new rollback entry
    pub fn new(step_id: impl Into<String>, tool_name: impl Into<String>, data: Value) -> Self {
        Self {
            step_id: step_id.into(),
            tool_name: tool_name.into(),
            rollback_data: data,
            created_at: Instant::now(),
        }
    }

    /// Get the age of this entry
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }
}

// =============================================================================
// Rollback Configuration
// =============================================================================

/// Configuration for the rollback manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackConfig {
    /// Whether rollback is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Maximum number of entries to keep
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,

    /// How long to keep entries (in seconds)
    #[serde(default = "default_retention_secs")]
    pub retention_secs: u64,
}

fn default_enabled() -> bool {
    true
}

fn default_max_entries() -> usize {
    100
}

fn default_retention_secs() -> u64 {
    3600 // 1 hour
}

impl Default for RollbackConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_entries: default_max_entries(),
            retention_secs: default_retention_secs(),
        }
    }
}

// =============================================================================
// Rollback Manager
// =============================================================================

/// Manager for rollback operations
///
/// Coordinates rollback of multiple steps in reverse order (LIFO).
pub struct RollbackManager {
    /// Registered rollback handlers: tool_name -> handler
    handlers: HashMap<String, Arc<dyn RollbackCapable>>,

    /// Pending rollback entries
    pending_entries: Arc<RwLock<Vec<RollbackEntry>>>,

    /// Configuration
    config: RollbackConfig,
}

impl RollbackManager {
    /// Create a new rollback manager with the given configuration
    pub fn new(config: RollbackConfig) -> Self {
        Self {
            handlers: HashMap::new(),
            pending_entries: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    /// Register a rollback handler for a tool
    pub fn register(&mut self, tool_name: &str, handler: Arc<dyn RollbackCapable>) {
        debug!(tool = %tool_name, "RollbackManager: Registering handler");
        self.handlers.insert(tool_name.to_string(), handler);
    }

    /// Check if a handler is registered for the given tool
    pub fn has_handler(&self, tool_name: &str) -> bool {
        self.handlers.contains_key(tool_name)
    }

    /// Add a rollback entry
    ///
    /// Respects max_entries configuration, removing oldest entries if needed.
    pub async fn add_entry(&self, entry: RollbackEntry) {
        if !self.config.enabled {
            debug!("RollbackManager: Rollback disabled, ignoring entry");
            return;
        }

        let mut entries = self.pending_entries.write().await;

        // Remove expired entries first
        let retention = Duration::from_secs(self.config.retention_secs);
        entries.retain(|e| e.age() < retention);

        // Remove oldest entries if at capacity
        while entries.len() >= self.config.max_entries {
            if !entries.is_empty() {
                let removed = entries.remove(0);
                debug!(
                    step_id = %removed.step_id,
                    tool = %removed.tool_name,
                    "RollbackManager: Removed oldest entry due to capacity"
                );
            }
        }

        info!(
            step_id = %entry.step_id,
            tool = %entry.tool_name,
            "RollbackManager: Added rollback entry"
        );
        entries.push(entry);
    }

    /// Execute rollback for all pending entries
    ///
    /// Entries are processed in reverse order (last in, first out).
    ///
    /// # Returns
    ///
    /// Vector of `RollbackResult` for each entry
    pub async fn rollback_all(&self) -> Vec<RollbackResult> {
        let mut entries = self.pending_entries.write().await;
        let mut results = Vec::new();

        // Process in reverse order (LIFO)
        while let Some(entry) = entries.pop() {
            let result = self.execute_rollback(&entry).await;
            results.push(result);
        }

        results
    }

    /// Rollback only the last entry
    ///
    /// # Returns
    ///
    /// `Some(RollbackResult)` if there was an entry to rollback, `None` otherwise
    pub async fn rollback_last(&self) -> Option<RollbackResult> {
        let mut entries = self.pending_entries.write().await;

        if let Some(entry) = entries.pop() {
            Some(self.execute_rollback(&entry).await)
        } else {
            None
        }
    }

    /// Clear all pending entries
    pub async fn clear(&self) {
        let mut entries = self.pending_entries.write().await;
        let count = entries.len();
        entries.clear();
        info!(count, "RollbackManager: Cleared all entries");
    }

    /// Get the number of pending entries
    pub async fn entry_count(&self) -> usize {
        let entries = self.pending_entries.read().await;
        entries.len()
    }

    /// Execute rollback for a single entry
    async fn execute_rollback(&self, entry: &RollbackEntry) -> RollbackResult {
        debug!(
            step_id = %entry.step_id,
            tool = %entry.tool_name,
            "RollbackManager: Attempting rollback"
        );

        // Check if handler exists
        let handler = match self.handlers.get(&entry.tool_name) {
            Some(h) => h,
            None => {
                info!(
                    step_id = %entry.step_id,
                    tool = %entry.tool_name,
                    "RollbackManager: No handler registered, skipping"
                );
                return RollbackResult::skipped("No rollback handler registered");
            }
        };

        // Execute rollback
        let result = handler.rollback(&entry.rollback_data).await;

        if result.success {
            info!(
                step_id = %entry.step_id,
                tool = %entry.tool_name,
                message = %result.message,
                "RollbackManager: Rollback succeeded"
            );
        } else {
            warn!(
                step_id = %entry.step_id,
                tool = %entry.tool_name,
                error = ?result.error,
                "RollbackManager: Rollback failed"
            );
        }

        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::sync_primitives::{AtomicUsize, Ordering};

    // Mock rollback handler for testing
    struct MockRollbackHandler {
        should_succeed: bool,
        call_count: AtomicUsize,
    }

    impl MockRollbackHandler {
        fn new(should_succeed: bool) -> Self {
            Self {
                should_succeed,
                call_count: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RollbackCapable for MockRollbackHandler {
        async fn rollback(&self, rollback_data: &Value) -> RollbackResult {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            if self.should_succeed {
                RollbackResult::success(format!("Rolled back with data: {:?}", rollback_data))
            } else {
                RollbackResult::failure("Mock rollback failure")
            }
        }

        fn rollback_description(&self) -> &str {
            "Mock rollback handler"
        }
    }

    // =============================================================================
    // RollbackResult Tests
    // =============================================================================

    #[test]
    fn test_rollback_result_success() {
        let result = RollbackResult::success("Rolled back file creation");
        assert!(result.success);
        assert_eq!(result.message, "Rolled back file creation");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_rollback_result_failure() {
        let result = RollbackResult::failure("File not found");
        assert!(!result.success);
        assert!(result.message.contains("Rollback failed"));
        assert_eq!(result.error, Some("File not found".to_string()));
    }

    #[test]
    fn test_rollback_result_skipped() {
        let result = RollbackResult::skipped("No handler registered");
        assert!(result.success);
        assert!(result.message.starts_with("Skipped:"));
        assert!(result.error.is_none());
    }

    // =============================================================================
    // RollbackEntry Tests
    // =============================================================================

    #[test]
    fn test_rollback_entry_creation() {
        let entry = RollbackEntry::new("step-1", "file_create", json!({"path": "/tmp/test.txt"}));

        assert_eq!(entry.step_id, "step-1");
        assert_eq!(entry.tool_name, "file_create");
        assert_eq!(entry.rollback_data["path"], "/tmp/test.txt");
    }

    #[test]
    fn test_rollback_entry_age() {
        let entry = RollbackEntry::new("step-1", "test", json!({}));

        // Age should be very small immediately after creation
        let age = entry.age();
        assert!(age.as_millis() < 100);

        // Simulate time passing (we can't really test this without sleeping)
        std::thread::sleep(Duration::from_millis(10));
        let new_age = entry.age();
        assert!(new_age >= age);
    }

    // =============================================================================
    // RollbackConfig Tests
    // =============================================================================

    #[test]
    fn test_rollback_config_defaults() {
        let config = RollbackConfig::default();

        assert!(config.enabled);
        assert_eq!(config.max_entries, 100);
        assert_eq!(config.retention_secs, 3600);
    }

    #[test]
    fn test_rollback_config_serialization() {
        let config = RollbackConfig {
            enabled: false,
            max_entries: 50,
            retention_secs: 1800,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: RollbackConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.max_entries, deserialized.max_entries);
        assert_eq!(config.retention_secs, deserialized.retention_secs);
    }

    // =============================================================================
    // RollbackManager Tests
    // =============================================================================

    #[test]
    fn test_manager_has_handler() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        assert!(!manager.has_handler("test_tool"));

        let handler = Arc::new(MockRollbackHandler::new(true));
        manager.register("test_tool", handler);

        assert!(manager.has_handler("test_tool"));
        assert!(!manager.has_handler("nonexistent"));
    }

    #[tokio::test]
    async fn test_manager_add_entry() {
        let manager = RollbackManager::new(RollbackConfig::default());

        assert_eq!(manager.entry_count().await, 0);

        let entry = RollbackEntry::new("step-1", "test", json!({}));
        manager.add_entry(entry).await;

        assert_eq!(manager.entry_count().await, 1);
    }

    #[tokio::test]
    async fn test_manager_add_entry_disabled() {
        let config = RollbackConfig {
            enabled: false,
            ..Default::default()
        };
        let manager = RollbackManager::new(config);

        let entry = RollbackEntry::new("step-1", "test", json!({}));
        manager.add_entry(entry).await;

        // Entry should not be added when disabled
        assert_eq!(manager.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_max_entries() {
        let config = RollbackConfig {
            max_entries: 3,
            ..Default::default()
        };
        let manager = RollbackManager::new(config);

        // Add 5 entries
        for i in 1..=5 {
            let entry = RollbackEntry::new(format!("step-{}", i), "test", json!({"index": i}));
            manager.add_entry(entry).await;
        }

        // Should only have 3 entries (the last 3)
        assert_eq!(manager.entry_count().await, 3);
    }

    #[tokio::test]
    async fn test_manager_rollback_all_success() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        let handler = Arc::new(MockRollbackHandler::new(true));
        manager.register("test_tool", handler.clone());

        // Add entries
        for i in 1..=3 {
            let entry = RollbackEntry::new(format!("step-{}", i), "test_tool", json!({"step": i}));
            manager.add_entry(entry).await;
        }

        let results = manager.rollback_all().await;

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.success));
        assert_eq!(handler.calls(), 3);

        // Entries should be cleared
        assert_eq!(manager.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_rollback_all_failure() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        let handler = Arc::new(MockRollbackHandler::new(false));
        manager.register("test_tool", handler);

        let entry = RollbackEntry::new("step-1", "test_tool", json!({}));
        manager.add_entry(entry).await;

        let results = manager.rollback_all().await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
    }

    #[tokio::test]
    async fn test_manager_rollback_no_handler() {
        let manager = RollbackManager::new(RollbackConfig::default());

        let entry = RollbackEntry::new("step-1", "unknown_tool", json!({}));
        manager.add_entry(entry).await;

        let results = manager.rollback_all().await;

        assert_eq!(results.len(), 1);
        // Skipped is considered success
        assert!(results[0].success);
        assert!(results[0].message.contains("Skipped"));
    }

    #[tokio::test]
    async fn test_manager_rollback_last() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        let handler = Arc::new(MockRollbackHandler::new(true));
        manager.register("test_tool", handler);

        // Add entries
        for i in 1..=3 {
            let entry = RollbackEntry::new(format!("step-{}", i), "test_tool", json!({"step": i}));
            manager.add_entry(entry).await;
        }

        // Rollback only last
        let result = manager.rollback_last().await;
        assert!(result.is_some());
        assert!(result.unwrap().success);

        // Should have 2 entries left
        assert_eq!(manager.entry_count().await, 2);
    }

    #[tokio::test]
    async fn test_manager_rollback_last_empty() {
        let manager = RollbackManager::new(RollbackConfig::default());

        let result = manager.rollback_last().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_manager_clear() {
        let manager = RollbackManager::new(RollbackConfig::default());

        for i in 1..=5 {
            let entry = RollbackEntry::new(format!("step-{}", i), "test", json!({}));
            manager.add_entry(entry).await;
        }

        assert_eq!(manager.entry_count().await, 5);

        manager.clear().await;

        assert_eq!(manager.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_rollback_reverse_order() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        // Track order of rollback calls
        struct OrderTrackingHandler {
            order: Arc<RwLock<Vec<i32>>>,
        }

        #[async_trait]
        impl RollbackCapable for OrderTrackingHandler {
            async fn rollback(&self, data: &Value) -> RollbackResult {
                if let Some(step) = data.get("step").and_then(|v| v.as_i64()) {
                    self.order.write().await.push(step as i32);
                }
                RollbackResult::success("OK")
            }

            fn rollback_description(&self) -> &str {
                "Order tracking handler"
            }
        }

        let order = Arc::new(RwLock::new(Vec::new()));
        let handler = Arc::new(OrderTrackingHandler {
            order: order.clone(),
        });
        manager.register("test_tool", handler);

        // Add entries in order 1, 2, 3
        for i in 1..=3 {
            let entry = RollbackEntry::new(format!("step-{}", i), "test_tool", json!({"step": i}));
            manager.add_entry(entry).await;
        }

        manager.rollback_all().await;

        // Should be rolled back in reverse order: 3, 2, 1
        let executed_order = order.read().await;
        assert_eq!(*executed_order, vec![3, 2, 1]);
    }

    #[tokio::test]
    async fn test_manager_mixed_handlers() {
        let mut manager = RollbackManager::new(RollbackConfig::default());

        let handler1 = Arc::new(MockRollbackHandler::new(true));
        let handler2 = Arc::new(MockRollbackHandler::new(true));
        manager.register("tool_a", handler1.clone());
        manager.register("tool_b", handler2.clone());

        // Add mixed entries
        manager
            .add_entry(RollbackEntry::new("1", "tool_a", json!({})))
            .await;
        manager
            .add_entry(RollbackEntry::new("2", "tool_b", json!({})))
            .await;
        manager
            .add_entry(RollbackEntry::new("3", "tool_a", json!({})))
            .await;
        manager
            .add_entry(RollbackEntry::new("4", "unknown", json!({})))
            .await; // No handler

        let results = manager.rollback_all().await;

        assert_eq!(results.len(), 4);
        // All should succeed (including skipped)
        assert!(results.iter().all(|r| r.success));

        // handler1 should be called twice, handler2 once
        assert_eq!(handler1.calls(), 2);
        assert_eq!(handler2.calls(), 1);
    }
}

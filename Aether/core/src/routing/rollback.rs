//! Rollback Support for Plan Execution
//!
//! This module provides rollback capabilities for reversible tool operations.
//!
//! # Architecture
//!
//! ```text
//! PlanExecutor
//!      ↓ (failure during execution)
//! ┌─────────────────────────────────────────┐
//! │           RollbackManager                │
//! │                                          │
//! │  For each completed step (reverse order):│
//! │    1. Check if step is Reversible        │
//! │    2. Get RollbackCapable handler        │
//! │    3. Execute rollback with saved data   │
//! │    4. Log rollback result                │
//! └──────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use crate::routing::rollback::{RollbackCapable, RollbackResult};
//!
//! #[async_trait]
//! impl RollbackCapable for FileCreateHandler {
//!     async fn rollback(&self, rollback_data: &Value) -> RollbackResult {
//!         // rollback_data contains the file path that was created
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
use std::sync::Arc;
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
        Self {
            success: false,
            message: String::new(),
            error: Some(error.into()),
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
/// the rollback method if execution fails.
///
/// # Example
///
/// ```rust,ignore
/// #[async_trait]
/// impl RollbackCapable for FileCopyHandler {
///     async fn rollback(&self, rollback_data: &Value) -> RollbackResult {
///         // rollback_data contains the destination path
///         if let Some(dest) = rollback_data.get("destination").and_then(|v| v.as_str()) {
///             match std::fs::remove_file(dest) {
///                 Ok(_) => RollbackResult::success(format!("Removed copy: {}", dest)),
///                 Err(e) => RollbackResult::failure(e.to_string()),
///             }
///         } else {
///             RollbackResult::failure("Missing destination in rollback data")
///         }
///     }
///
///     fn rollback_description(&self) -> &str {
///         "Removes the copied file"
///     }
/// }
/// ```
#[async_trait]
pub trait RollbackCapable: Send + Sync {
    /// Perform the rollback operation
    ///
    /// # Arguments
    ///
    /// * `rollback_data` - Data collected during the original execution
    ///
    /// # Returns
    ///
    /// `RollbackResult` indicating success or failure
    async fn rollback(&self, rollback_data: &Value) -> RollbackResult;

    /// Get a description of what the rollback does
    ///
    /// Used for logging and UI display.
    fn rollback_description(&self) -> &str;

    /// Check if rollback is possible with the given data
    ///
    /// Default implementation returns true. Override to add validation.
    fn can_rollback(&self, _rollback_data: &Value) -> bool {
        true
    }
}

// =============================================================================
// Rollback Registry
// =============================================================================

/// Registry for rollback-capable handlers
///
/// Maps tool names to their rollback handlers.
pub struct RollbackRegistry {
    /// Handlers: tool_name -> RollbackCapable
    handlers: Arc<RwLock<HashMap<String, Arc<dyn RollbackCapable>>>>,
}

impl Default for RollbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RollbackRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a rollback handler for a tool
    pub async fn register(&self, tool_name: impl Into<String>, handler: Arc<dyn RollbackCapable>) {
        let mut handlers = self.handlers.write().await;
        handlers.insert(tool_name.into(), handler);
    }

    /// Get a rollback handler for a tool
    pub async fn get(&self, tool_name: &str) -> Option<Arc<dyn RollbackCapable>> {
        let handlers = self.handlers.read().await;
        handlers.get(tool_name).cloned()
    }

    /// Check if a tool has a rollback handler
    pub async fn has_handler(&self, tool_name: &str) -> bool {
        let handlers = self.handlers.read().await;
        handlers.contains_key(tool_name)
    }

    /// Get all registered tool names
    pub async fn tool_names(&self) -> Vec<String> {
        let handlers = self.handlers.read().await;
        handlers.keys().cloned().collect()
    }
}

// =============================================================================
// Rollback Entry
// =============================================================================

/// Entry storing rollback information for a completed step
#[derive(Debug, Clone)]
pub struct RollbackEntry {
    /// Step index (1-based)
    pub step_index: u32,

    /// Tool name
    pub tool_name: String,

    /// Rollback data collected during execution
    pub rollback_data: Value,
}

impl RollbackEntry {
    /// Create a new rollback entry
    pub fn new(step_index: u32, tool_name: impl Into<String>, rollback_data: Value) -> Self {
        Self {
            step_index,
            tool_name: tool_name.into(),
            rollback_data,
        }
    }
}

// =============================================================================
// Rollback Manager
// =============================================================================

/// Manager for executing rollback operations
///
/// Coordinates rollback of multiple steps in reverse order.
pub struct RollbackManager {
    /// Registry of rollback handlers
    registry: Arc<RollbackRegistry>,
}

impl RollbackManager {
    /// Create a new rollback manager
    pub fn new(registry: Arc<RollbackRegistry>) -> Self {
        Self { registry }
    }

    /// Execute rollback for a list of entries
    ///
    /// Entries are processed in reverse order (last completed step first).
    ///
    /// # Arguments
    ///
    /// * `entries` - List of rollback entries to process
    ///
    /// # Returns
    ///
    /// Vector of (step_index, RollbackResult) pairs
    pub async fn execute_rollback(
        &self,
        entries: &[RollbackEntry],
    ) -> Vec<(u32, RollbackResult)> {
        let mut results = Vec::new();

        // Process in reverse order
        for entry in entries.iter().rev() {
            let result = self.rollback_step(entry).await;
            results.push((entry.step_index, result));
        }

        results
    }

    /// Rollback a single step
    async fn rollback_step(&self, entry: &RollbackEntry) -> RollbackResult {
        debug!(
            step = entry.step_index,
            tool = %entry.tool_name,
            "RollbackManager: Attempting rollback"
        );

        // Check if handler exists
        let handler = match self.registry.get(&entry.tool_name).await {
            Some(h) => h,
            None => {
                info!(
                    step = entry.step_index,
                    tool = %entry.tool_name,
                    "RollbackManager: No handler registered, skipping"
                );
                return RollbackResult::skipped("No rollback handler registered");
            }
        };

        // Check if rollback is possible
        if !handler.can_rollback(&entry.rollback_data) {
            info!(
                step = entry.step_index,
                tool = %entry.tool_name,
                "RollbackManager: Cannot rollback with provided data"
            );
            return RollbackResult::skipped("Rollback not possible with provided data");
        }

        // Execute rollback
        match handler.rollback(&entry.rollback_data).await {
            result if result.success => {
                info!(
                    step = entry.step_index,
                    tool = %entry.tool_name,
                    message = %result.message,
                    "RollbackManager: Rollback succeeded"
                );
                result
            }
            result => {
                warn!(
                    step = entry.step_index,
                    tool = %entry.tool_name,
                    error = ?result.error,
                    "RollbackManager: Rollback failed"
                );
                result
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Mock rollback handler for testing
    struct MockRollbackHandler {
        should_succeed: bool,
    }

    #[async_trait]
    impl RollbackCapable for MockRollbackHandler {
        async fn rollback(&self, rollback_data: &Value) -> RollbackResult {
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

    #[test]
    fn test_rollback_result_success() {
        let result = RollbackResult::success("Rolled back file creation");
        assert!(result.success);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_rollback_result_failure() {
        let result = RollbackResult::failure("File not found");
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_rollback_result_skipped() {
        let result = RollbackResult::skipped("No handler registered");
        assert!(result.success);
        assert!(result.message.starts_with("Skipped:"));
    }

    #[tokio::test]
    async fn test_rollback_registry() {
        let registry = RollbackRegistry::new();

        // Register handler
        let handler = Arc::new(MockRollbackHandler { should_succeed: true });
        registry.register("test_tool", handler).await;

        // Check registration
        assert!(registry.has_handler("test_tool").await);
        assert!(!registry.has_handler("nonexistent").await);

        // Get handler
        let retrieved = registry.get("test_tool").await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_rollback_manager_success() {
        let registry = Arc::new(RollbackRegistry::new());
        let handler = Arc::new(MockRollbackHandler { should_succeed: true });
        registry.register("test_tool", handler).await;

        let manager = RollbackManager::new(registry);

        let entries = vec![
            RollbackEntry::new(1, "test_tool", json!({"path": "/tmp/test"})),
        ];

        let results = manager.execute_rollback(&entries).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 1);
        assert!(results[0].1.success);
    }

    #[tokio::test]
    async fn test_rollback_manager_failure() {
        let registry = Arc::new(RollbackRegistry::new());
        let handler = Arc::new(MockRollbackHandler { should_succeed: false });
        registry.register("test_tool", handler).await;

        let manager = RollbackManager::new(registry);

        let entries = vec![
            RollbackEntry::new(1, "test_tool", json!({})),
        ];

        let results = manager.execute_rollback(&entries).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].1.success);
    }

    #[tokio::test]
    async fn test_rollback_manager_no_handler() {
        let registry = Arc::new(RollbackRegistry::new());
        let manager = RollbackManager::new(registry);

        let entries = vec![
            RollbackEntry::new(1, "unknown_tool", json!({})),
        ];

        let results = manager.execute_rollback(&entries).await;
        assert_eq!(results.len(), 1);
        // Skipped is considered success
        assert!(results[0].1.success);
        assert!(results[0].1.message.contains("Skipped"));
    }

    #[tokio::test]
    async fn test_rollback_manager_reverse_order() {
        let registry = Arc::new(RollbackRegistry::new());
        let handler = Arc::new(MockRollbackHandler { should_succeed: true });
        registry.register("tool", handler).await;

        let manager = RollbackManager::new(registry);

        let entries = vec![
            RollbackEntry::new(1, "tool", json!({"step": 1})),
            RollbackEntry::new(2, "tool", json!({"step": 2})),
            RollbackEntry::new(3, "tool", json!({"step": 3})),
        ];

        let results = manager.execute_rollback(&entries).await;

        // Should be in reverse order
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, 3); // Last step rolled back first
        assert_eq!(results[1].0, 2);
        assert_eq!(results[2].0, 1);
    }

    #[test]
    fn test_rollback_entry() {
        let entry = RollbackEntry::new(1, "file_create", json!({"path": "/tmp/test.txt"}));

        assert_eq!(entry.step_index, 1);
        assert_eq!(entry.tool_name, "file_create");
        assert_eq!(entry.rollback_data["path"], "/tmp/test.txt");
    }
}

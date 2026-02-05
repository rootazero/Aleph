//! Recursion depth tracking for sub-agent spawning
//!
//! Prevents infinite nesting by tracking parent-child relationships
//! and enforcing a maximum recursion depth limit.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::error::{AlephError, Result};

/// Tracks recursion depth to prevent infinite nesting
pub struct RecursionTracker {
    /// Maps child session key to parent session key
    parent_map: Arc<RwLock<HashMap<String, String>>>,
    /// Maximum allowed recursion depth
    max_depth: usize,
}

impl RecursionTracker {
    /// Create a new RecursionTracker with the given max depth
    pub fn new(max_depth: usize) -> Self {
        Self {
            parent_map: Arc::new(RwLock::new(HashMap::new())),
            max_depth,
        }
    }

    /// Record a parent-child spawn relationship
    pub async fn track_spawn(&self, parent_run_id: &str, child_run_id: &str) {
        let mut map = self.parent_map.write().await;
        map.insert(child_run_id.to_string(), parent_run_id.to_string());
    }

    /// Get the recursion depth for a given run_id
    ///
    /// Depth is calculated by traversing the parent chain.
    /// A run with no parent has depth 0.
    pub async fn get_depth(&self, run_id: &str) -> usize {
        let map = self.parent_map.read().await;
        let mut depth = 0;
        let mut current = run_id.to_string();

        while let Some(parent) = map.get(&current) {
            depth += 1;
            current = parent.clone();

            // Safety check to prevent infinite loops
            if depth > self.max_depth {
                break;
            }
        }

        depth
    }

    /// Check if a parent can spawn a child without exceeding max depth
    pub async fn can_spawn(&self, parent_run_id: &str, max_depth: usize) -> Result<()> {
        let current_depth = self.get_depth(parent_run_id).await;

        if current_depth >= max_depth {
            return Err(AlephError::config(format!(
                "Recursion depth limit reached: {} >= {}",
                current_depth, max_depth
            )));
        }

        Ok(())
    }

    /// Remove a run from tracking (cleanup on completion)
    pub async fn remove(&self, run_id: &str) {
        let mut map = self.parent_map.write().await;
        map.remove(run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_recursion_depth_tracking() {
        let tracker = RecursionTracker::new(5);

        // Root run has depth 0
        assert_eq!(tracker.get_depth("parent-1").await, 0);

        // Track first spawn
        assert!(tracker.can_spawn("parent-1", 5).await.is_ok());
        tracker.track_spawn("parent-1", "child-1").await;

        assert_eq!(tracker.get_depth("child-1").await, 1);

        // Track second level spawn
        assert!(tracker.can_spawn("child-1", 5).await.is_ok());
        tracker.track_spawn("child-1", "child-2").await;

        assert_eq!(tracker.get_depth("child-2").await, 2);
    }

    #[tokio::test]
    async fn test_recursion_depth_limit() {
        let tracker = RecursionTracker::new(3);

        tracker.track_spawn("p0", "p1").await;
        tracker.track_spawn("p1", "p2").await;
        tracker.track_spawn("p2", "p3").await;

        // p3 is at depth 3, which equals the limit
        assert_eq!(tracker.get_depth("p3").await, 3);

        // Should not be able to spawn from p3
        let result = tracker.can_spawn("p3", 3).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_recursion_depth_zero() {
        let tracker = RecursionTracker::new(5);

        // Untracked runs have depth 0
        assert_eq!(tracker.get_depth("unknown").await, 0);
    }

    #[tokio::test]
    async fn test_recursion_remove() {
        let tracker = RecursionTracker::new(5);

        tracker.track_spawn("parent", "child").await;
        assert_eq!(tracker.get_depth("child").await, 1);

        tracker.remove("child").await;
        assert_eq!(tracker.get_depth("child").await, 0);
    }

    #[tokio::test]
    async fn test_recursion_multiple_children() {
        let tracker = RecursionTracker::new(5);

        // One parent can have multiple children
        tracker.track_spawn("parent", "child-1").await;
        tracker.track_spawn("parent", "child-2").await;
        tracker.track_spawn("parent", "child-3").await;

        assert_eq!(tracker.get_depth("child-1").await, 1);
        assert_eq!(tracker.get_depth("child-2").await, 1);
        assert_eq!(tracker.get_depth("child-3").await, 1);
    }

    #[tokio::test]
    async fn test_recursion_long_chain() {
        let tracker = RecursionTracker::new(10);

        // Build a chain: p0 -> p1 -> p2 -> ... -> p5
        tracker.track_spawn("p0", "p1").await;
        tracker.track_spawn("p1", "p2").await;
        tracker.track_spawn("p2", "p3").await;
        tracker.track_spawn("p3", "p4").await;
        tracker.track_spawn("p4", "p5").await;

        assert_eq!(tracker.get_depth("p0").await, 0);
        assert_eq!(tracker.get_depth("p1").await, 1);
        assert_eq!(tracker.get_depth("p2").await, 2);
        assert_eq!(tracker.get_depth("p3").await, 3);
        assert_eq!(tracker.get_depth("p4").await, 4);
        assert_eq!(tracker.get_depth("p5").await, 5);

        // Should still be able to spawn from p5 (depth 5 < max 10)
        assert!(tracker.can_spawn("p5", 10).await.is_ok());
    }

    #[tokio::test]
    async fn test_recursion_can_spawn_at_limit_minus_one() {
        let tracker = RecursionTracker::new(3);

        tracker.track_spawn("p0", "p1").await;
        tracker.track_spawn("p1", "p2").await;

        // p2 is at depth 2, should be able to spawn one more level
        assert!(tracker.can_spawn("p2", 3).await.is_ok());

        tracker.track_spawn("p2", "p3").await;

        // p3 is at depth 3 (at limit), cannot spawn more
        assert!(tracker.can_spawn("p3", 3).await.is_err());
    }
}

//! Recursive Sentry
//!
//! Implements recursion depth tracking and circuit breaker
//! to prevent infinite task spawning loops.

use crate::error::AlephError;
use crate::resilience::database::StateDatabase;
use std::sync::Arc;
use tracing::{debug, warn};

/// Recursion limit exceeded error
#[derive(Debug, Clone)]
pub struct RecursionLimitExceeded {
    /// Task that exceeded the limit
    pub task_id: String,

    /// Current depth
    pub depth: u32,

    /// Maximum allowed depth
    pub max_depth: u32,

    /// Parent task chain
    pub parent_chain: Vec<String>,
}

impl std::fmt::Display for RecursionLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Recursion limit exceeded: task {} at depth {} (max {}). Chain: {:?}",
            self.task_id, self.depth, self.max_depth, self.parent_chain
        )
    }
}

impl std::error::Error for RecursionLimitExceeded {}

/// Recursive Sentry for depth tracking
///
/// Tracks task spawning depth and enforces limits to prevent
/// runaway recursive task creation.
pub struct RecursiveSentry {
    db: Arc<StateDatabase>,
    max_depth: u32,
}

impl RecursiveSentry {
    /// Create a new Recursive Sentry
    pub fn new(db: Arc<StateDatabase>, max_depth: u32) -> Self {
        Self { db, max_depth }
    }

    /// Check if a new task can be spawned at the given depth
    pub fn check_depth(&self, depth: u32) -> Result<(), RecursionLimitExceeded> {
        if depth > self.max_depth {
            Err(RecursionLimitExceeded {
                task_id: String::new(),
                depth,
                max_depth: self.max_depth,
                parent_chain: Vec::new(),
            })
        } else {
            Ok(())
        }
    }

    /// Calculate the depth for a child task
    pub async fn calculate_child_depth(
        &self,
        parent_task_id: Option<&str>,
    ) -> Result<u32, AlephError> {
        match parent_task_id {
            Some(parent_id) => {
                // Get parent task to find its depth
                let parent = self.db.get_agent_task(parent_id).await?;

                match parent {
                    Some(task) => {
                        let child_depth = task.recursion_depth + 1;
                        debug!(
                            parent_id = %parent_id,
                            parent_depth = task.recursion_depth,
                            child_depth = child_depth,
                            "Calculated child depth"
                        );
                        Ok(child_depth)
                    }
                    None => {
                        // Parent not found, assume root
                        warn!(
                            parent_id = %parent_id,
                            "Parent task not found, assuming root depth"
                        );
                        Ok(0)
                    }
                }
            }
            None => {
                // No parent, this is a root task
                Ok(0)
            }
        }
    }

    /// Validate and calculate depth for a new task
    ///
    /// Returns the depth if valid, or an error if limit exceeded.
    pub async fn validate_spawn(
        &self,
        parent_task_id: Option<&str>,
    ) -> Result<u32, RecursionLimitExceeded> {
        let depth = self
            .calculate_child_depth(parent_task_id)
            .await
            .map_err(|e| RecursionLimitExceeded {
                task_id: String::new(),
                depth: 0,
                max_depth: self.max_depth,
                parent_chain: vec![format!("Error: {}", e)],
            })?;

        if depth > self.max_depth {
            // Build parent chain for debugging
            let parent_chain = self
                .build_parent_chain(parent_task_id)
                .await
                .unwrap_or_default();

            warn!(
                depth = depth,
                max_depth = self.max_depth,
                parent_chain = ?parent_chain,
                "Recursion limit would be exceeded"
            );

            return Err(RecursionLimitExceeded {
                task_id: String::new(),
                depth,
                max_depth: self.max_depth,
                parent_chain,
            });
        }

        Ok(depth)
    }

    /// Build the chain of parent task IDs
    async fn build_parent_chain(
        &self,
        start_task_id: Option<&str>,
    ) -> Result<Vec<String>, AlephError> {
        let mut chain = Vec::new();
        let mut current_id = start_task_id.map(|s| s.to_string());

        // Limit chain traversal to prevent infinite loops
        let max_chain_length = self.max_depth as usize + 5;

        while let Some(id) = current_id {
            if chain.len() >= max_chain_length {
                chain.push("...".to_string());
                break;
            }

            chain.push(id.clone());

            let task = self.db.get_agent_task(&id).await?;
            current_id = task.and_then(|t| t.parent_task_id);
        }

        Ok(chain)
    }

    /// Get the maximum depth
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Check if depth is approaching limit
    pub fn is_near_limit(&self, depth: u32) -> bool {
        depth >= self.max_depth.saturating_sub(1)
    }

    /// Get remaining depth budget
    pub fn remaining_depth(&self, current_depth: u32) -> u32 {
        self.max_depth.saturating_sub(current_depth)
    }
}

impl std::fmt::Debug for RecursiveSentry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecursiveSentry")
            .field("max_depth", &self.max_depth)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recursion_limit_exceeded_display() {
        let err = RecursionLimitExceeded {
            task_id: "task_123".to_string(),
            depth: 5,
            max_depth: 3,
            parent_chain: vec!["task_1".to_string(), "task_2".to_string()],
        };

        let msg = err.to_string();
        assert!(msg.contains("task_123"));
        assert!(msg.contains("5"));
        assert!(msg.contains("3"));
    }

    #[test]
    fn test_check_depth() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_sentry_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(StateDatabase::new(db_path).unwrap());

        let sentry = RecursiveSentry::new(db, 3);

        assert!(sentry.check_depth(0).is_ok());
        assert!(sentry.check_depth(3).is_ok());
        assert!(sentry.check_depth(4).is_err());
    }

    #[test]
    fn test_remaining_depth() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_sentry2_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(StateDatabase::new(db_path).unwrap());

        let sentry = RecursiveSentry::new(db, 3);

        assert_eq!(sentry.remaining_depth(0), 3);
        assert_eq!(sentry.remaining_depth(2), 1);
        assert_eq!(sentry.remaining_depth(3), 0);
        assert_eq!(sentry.remaining_depth(5), 0);
    }

    #[test]
    fn test_is_near_limit() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_sentry3_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(StateDatabase::new(db_path).unwrap());

        let sentry = RecursiveSentry::new(db, 3);

        assert!(!sentry.is_near_limit(0));
        assert!(!sentry.is_near_limit(1));
        assert!(sentry.is_near_limit(2));
        assert!(sentry.is_near_limit(3));
    }
}

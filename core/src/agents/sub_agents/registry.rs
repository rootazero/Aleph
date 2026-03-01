//! Sub-Agent Registry
//!
//! This module provides in-memory indexing and lifecycle event broadcasting
//! for sub-agent run instances as part of the Multi-Agent 2.0 system.
//!
//! # Overview
//!
//! The `SubAgentRegistry` manages all sub-agent runs with:
//! - Primary index by run_id
//! - Secondary index by session key
//! - Parent-child relationship tracking
//! - Lifecycle event broadcasting
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::agents::sub_agents::{SubAgentRegistry, SubAgentRun, LifecycleEvent};
//!
//! let registry = SubAgentRegistry::new_in_memory();
//!
//! // Subscribe to lifecycle events
//! let mut rx = registry.subscribe();
//!
//! // Register a new run
//! let run = SubAgentRun::new(session_key, parent_key, "Task", "explore");
//! let run_id = registry.register(run).await?;
//!
//! // Query by various indices
//! let run = registry.get(&run_id).await?;
//! let run_id = registry.get_by_session(&session_key).await;
//! let children = registry.get_children(&parent_key).await;
//! ```

use std::collections::HashMap;

use tokio::sync::{broadcast, RwLock};

use super::run::{RunStatus, SubAgentRun};
use crate::error::Result;
use crate::routing::SessionKey;

/// Registry statistics
#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub paused: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

/// Lifecycle events emitted by the registry
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// A new run was registered
    Registered { run_id: String },
    /// A run's status changed
    StatusChanged {
        run_id: String,
        old: RunStatus,
        new: RunStatus,
    },
}

/// Default broadcast channel capacity
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Sub-Agent Registry for managing run instances
///
/// Provides in-memory indexing with multiple access patterns:
/// - By run_id (primary key)
/// - By session key (for session-to-run lookup)
/// - By parent session key (for parent-child relationships)
pub struct SubAgentRegistry {
    /// Primary index: run_id -> SubAgentRun
    runs: RwLock<HashMap<String, SubAgentRun>>,
    /// Secondary index: session_key -> run_id
    by_session: RwLock<HashMap<SessionKey, String>>,
    /// Parent-child index: parent_session_key -> Vec<run_id>
    by_parent: RwLock<HashMap<SessionKey, Vec<String>>>,
    /// Broadcast channel for lifecycle events
    event_tx: broadcast::Sender<LifecycleEvent>,
}

impl SubAgentRegistry {
    /// Create a new in-memory registry with default broadcast channel capacity
    pub fn new_in_memory() -> Self {
        let (event_tx, _) = broadcast::channel(DEFAULT_CHANNEL_CAPACITY);
        Self {
            runs: RwLock::new(HashMap::new()),
            by_session: RwLock::new(HashMap::new()),
            by_parent: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Subscribe to lifecycle events
    pub fn subscribe(&self) -> broadcast::Receiver<LifecycleEvent> {
        self.event_tx.subscribe()
    }

    /// Register a new sub-agent run
    ///
    /// Updates all indices and emits a `Registered` lifecycle event.
    ///
    /// # Arguments
    ///
    /// * `run` - The sub-agent run to register
    ///
    /// # Returns
    ///
    /// The run_id of the registered run
    pub async fn register(&self, run: SubAgentRun) -> Result<String> {
        let run_id = run.run_id.clone();
        let session_key = run.session_key.clone();
        let parent_key = run.parent_session_key.clone();

        // Update primary index
        {
            let mut runs = self.runs.write().await;
            runs.insert(run_id.clone(), run);
        }

        // Update session index
        {
            let mut by_session = self.by_session.write().await;
            by_session.insert(session_key, run_id.clone());
        }

        // Update parent-child index
        {
            let mut by_parent = self.by_parent.write().await;
            by_parent
                .entry(parent_key)
                .or_insert_with(Vec::new)
                .push(run_id.clone());
        }

        // Emit lifecycle event (ignore send errors if no receivers)
        let _ = self.event_tx.send(LifecycleEvent::Registered {
            run_id: run_id.clone(),
        });

        Ok(run_id)
    }

    /// Get a sub-agent run by its run_id
    ///
    /// # Arguments
    ///
    /// * `run_id` - The unique identifier of the run
    ///
    /// # Returns
    ///
    /// The run if found, None otherwise
    pub async fn get(&self, run_id: &str) -> Result<Option<SubAgentRun>> {
        let runs = self.runs.read().await;
        Ok(runs.get(run_id).cloned())
    }

    /// Get a run_id by session key
    ///
    /// # Arguments
    ///
    /// * `key` - The session key to look up
    ///
    /// # Returns
    ///
    /// The run_id if found, None otherwise
    pub async fn get_by_session(&self, key: &SessionKey) -> Option<String> {
        let by_session = self.by_session.read().await;
        by_session.get(key).cloned()
    }

    /// Get all child run_ids for a parent session key
    ///
    /// # Arguments
    ///
    /// * `parent` - The parent session key
    ///
    /// # Returns
    ///
    /// A vector of run_ids for all children of the parent
    pub async fn get_children(&self, parent: &SessionKey) -> Vec<String> {
        let by_parent = self.by_parent.read().await;
        by_parent.get(parent).cloned().unwrap_or_default()
    }

    /// Transition a run to a new status
    ///
    /// Validates the transition, updates timestamps, and emits a lifecycle event.
    ///
    /// # Arguments
    ///
    /// * `run_id` - The unique identifier of the run
    /// * `new_status` - The target status to transition to
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The run is not found
    /// - The transition is invalid (e.g., Pending -> Completed)
    pub async fn transition(&self, run_id: &str, new_status: RunStatus) -> Result<()> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id).ok_or_else(|| {
            crate::error::AlephError::config(format!("Run not found: {}", run_id))
        })?;

        let old_status = run.status;
        if !old_status.can_transition_to(&new_status) {
            return Err(crate::error::AlephError::config(format!(
                "Invalid transition: {:?} -> {:?}",
                old_status, new_status
            )));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Update timestamps based on transition
        match new_status {
            RunStatus::Running => run.started_at = Some(now),
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled => {
                run.ended_at = Some(now);
            }
            _ => {}
        }

        run.status = new_status;

        let _ = self.event_tx.send(LifecycleEvent::StatusChanged {
            run_id: run_id.to_string(),
            old: old_status,
            new: new_status,
        });

        Ok(())
    }

    /// Get all active (non-terminal) runs
    ///
    /// Returns runs that are not in a terminal state (Completed, Failed, Cancelled).
    pub async fn get_active_runs(&self) -> Vec<SubAgentRun> {
        self.runs
            .read()
            .await
            .values()
            .filter(|r| !r.status.is_terminal())
            .cloned()
            .collect()
    }

    /// Get runs by status
    ///
    /// # Arguments
    ///
    /// * `status` - The status to filter by
    ///
    /// # Returns
    ///
    /// A vector of runs with the specified status
    pub async fn get_by_status(&self, status: RunStatus) -> Vec<SubAgentRun> {
        self.runs
            .read()
            .await
            .values()
            .filter(|r| r.status == status)
            .cloned()
            .collect()
    }

    /// Get statistics about the registry
    ///
    /// Returns counts of runs in each status category.
    pub async fn stats(&self) -> RegistryStats {
        let runs = self.runs.read().await;
        let mut stats = RegistryStats::default();

        for run in runs.values() {
            stats.total += 1;
            match run.status {
                RunStatus::Pending => stats.pending += 1,
                RunStatus::Running => stats.running += 1,
                RunStatus::Paused => stats.paused += 1,
                RunStatus::Completed => stats.completed += 1,
                RunStatus::Failed => stats.failed += 1,
                RunStatus::Cancelled => stats.cancelled += 1,
            }
        }

        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session_key(id: &str) -> SessionKey {
        SessionKey::main(id)
    }

    fn make_subagent_key(parent_id: &str, subagent_id: &str) -> SessionKey {
        SessionKey::Subagent {
            parent_key: Box::new(SessionKey::main(parent_id)),
            subagent_id: subagent_id.to_string(),
        }
    }

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = SubAgentRegistry::new_in_memory();
        let session_key = make_subagent_key("parent-1", "session-1");
        let parent_key = make_session_key("parent-1");
        let run = SubAgentRun::new(session_key, parent_key, "Test task", "explore");
        let run_id = run.run_id.clone();

        registry.register(run).await.unwrap();

        let retrieved = registry.get(&run_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().task, "Test task");
    }

    #[tokio::test]
    async fn test_registry_get_by_session() {
        let registry = SubAgentRegistry::new_in_memory();
        let session_key = make_subagent_key("parent-1", "session-abc");
        let parent_key = make_session_key("parent-1");
        let run = SubAgentRun::new(session_key.clone(), parent_key, "Task", "plan");
        let run_id = run.run_id.clone();

        registry.register(run).await.unwrap();

        let found = registry.get_by_session(&session_key).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap(), run_id);
    }

    #[tokio::test]
    async fn test_registry_get_children() {
        let registry = SubAgentRegistry::new_in_memory();

        let parent_x = make_session_key("parent-x");
        let parent_y = make_session_key("parent-y");

        let run1 = SubAgentRun::new(
            make_subagent_key("parent-x", "s1"),
            parent_x.clone(),
            "Task 1",
            "explore",
        );
        let run2 = SubAgentRun::new(
            make_subagent_key("parent-x", "s2"),
            parent_x.clone(),
            "Task 2",
            "plan",
        );
        let run3 = SubAgentRun::new(
            make_subagent_key("parent-y", "s3"),
            parent_y.clone(),
            "Task 3",
            "execute",
        );

        registry.register(run1).await.unwrap();
        registry.register(run2).await.unwrap();
        registry.register(run3).await.unwrap();

        let children = registry.get_children(&parent_x).await;
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn test_registry_transition() {
        let registry = SubAgentRegistry::new_in_memory();
        let session_key = make_subagent_key("p1", "s1");
        let parent_key = make_session_key("p1");
        let run = SubAgentRun::new(session_key, parent_key, "Task", "explore");
        let run_id = run.run_id.clone();
        registry.register(run).await.unwrap();

        // Transition to Running
        registry
            .transition(&run_id, RunStatus::Running)
            .await
            .unwrap();
        let run = registry.get(&run_id).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.started_at.is_some());

        // Transition to Completed
        registry
            .transition(&run_id, RunStatus::Completed)
            .await
            .unwrap();
        let run = registry.get(&run_id).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Completed);
        assert!(run.ended_at.is_some());
    }

    #[tokio::test]
    async fn test_registry_invalid_transition() {
        let registry = SubAgentRegistry::new_in_memory();
        let session_key = make_subagent_key("p1", "s1");
        let parent_key = make_session_key("p1");
        let run = SubAgentRun::new(session_key, parent_key, "Task", "explore");
        let run_id = run.run_id.clone();
        registry.register(run).await.unwrap();

        // Invalid: Pending -> Completed (must go through Running)
        let result = registry.transition(&run_id, RunStatus::Completed).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_active_runs() {
        let registry = SubAgentRegistry::new_in_memory();
        let run1 = SubAgentRun::new(
            make_subagent_key("p1", "s1"),
            make_session_key("p1"),
            "Task 1",
            "explore",
        );
        let run2 = SubAgentRun::new(
            make_subagent_key("p1", "s2"),
            make_session_key("p1"),
            "Task 2",
            "plan",
        );

        let run1_id = run1.run_id.clone();
        let run2_id = run2.run_id.clone();

        registry.register(run1).await.unwrap();
        registry.register(run2).await.unwrap();
        registry
            .transition(&run1_id, RunStatus::Running)
            .await
            .unwrap();
        registry
            .transition(&run2_id, RunStatus::Running)
            .await
            .unwrap();
        registry
            .transition(&run2_id, RunStatus::Completed)
            .await
            .unwrap();

        let active = registry.get_active_runs().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].run_id, run1_id);
    }

    #[tokio::test]
    async fn test_registry_stats() {
        let registry = SubAgentRegistry::new_in_memory();

        let run1 = SubAgentRun::new(
            make_subagent_key("p1", "s1"),
            make_session_key("p1"),
            "Task 1",
            "explore",
        );
        let run2 = SubAgentRun::new(
            make_subagent_key("p1", "s2"),
            make_session_key("p1"),
            "Task 2",
            "plan",
        );
        let run3 = SubAgentRun::new(
            make_subagent_key("p1", "s3"),
            make_session_key("p1"),
            "Task 3",
            "execute",
        );

        let run1_id = run1.run_id.clone();
        let run2_id = run2.run_id.clone();

        registry.register(run1).await.unwrap();
        registry.register(run2).await.unwrap();
        registry.register(run3).await.unwrap();

        registry
            .transition(&run1_id, RunStatus::Running)
            .await
            .unwrap();
        registry
            .transition(&run2_id, RunStatus::Running)
            .await
            .unwrap();
        registry
            .transition(&run2_id, RunStatus::Completed)
            .await
            .unwrap();

        let stats = registry.stats().await;
        assert_eq!(stats.total, 3);
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.completed, 1);
    }

    #[tokio::test]
    async fn test_get_by_status() {
        let registry = SubAgentRegistry::new_in_memory();

        let run1 = SubAgentRun::new(
            make_subagent_key("p1", "s1"),
            make_session_key("p1"),
            "Task 1",
            "explore",
        );
        let run2 = SubAgentRun::new(
            make_subagent_key("p1", "s2"),
            make_session_key("p1"),
            "Task 2",
            "plan",
        );

        let run1_id = run1.run_id.clone();

        registry.register(run1).await.unwrap();
        registry.register(run2).await.unwrap();
        registry
            .transition(&run1_id, RunStatus::Running)
            .await
            .unwrap();

        let pending = registry.get_by_status(RunStatus::Pending).await;
        assert_eq!(pending.len(), 1);

        let running = registry.get_by_status(RunStatus::Running).await;
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].run_id, run1_id);
    }
}

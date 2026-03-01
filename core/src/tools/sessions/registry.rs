//! Sub-agent run registry.
//!
//! Tracks active sub-agent runs for cleanup and result announcement.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Information about a spawned sub-agent run
#[derive(Debug, Clone)]
pub struct SubagentRun {
    /// Unique run ID
    pub run_id: String,
    /// Child session key
    pub child_session_key: String,
    /// Parent (requester) session key
    pub requester_session_key: String,
    /// Task description
    pub task: String,
    /// Cleanup policy: "keep" or "delete"
    pub cleanup: String,
    /// Optional label
    pub label: Option<String>,
    /// Start timestamp
    pub started_at: i64,
}

/// Registry for tracking active sub-agent runs
#[derive(Debug, Clone, Default)]
pub struct SubagentRegistry {
    runs: Arc<RwLock<HashMap<String, SubagentRun>>>,
}

impl SubagentRegistry {
    /// Create a new registry
    pub fn new() -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new sub-agent run
    pub async fn register(&self, run: SubagentRun) {
        let mut runs = self.runs.write().await;
        runs.insert(run.run_id.clone(), run);
    }

    /// Get a run by ID
    pub async fn get(&self, run_id: &str) -> Option<SubagentRun> {
        let runs = self.runs.read().await;
        runs.get(run_id).cloned()
    }

    /// Remove and return a run by ID
    pub async fn remove(&self, run_id: &str) -> Option<SubagentRun> {
        let mut runs = self.runs.write().await;
        runs.remove(run_id)
    }

    /// List runs by requester session key
    pub async fn list_by_requester(&self, requester_key: &str) -> Vec<SubagentRun> {
        let runs = self.runs.read().await;
        runs.values()
            .filter(|r| r.requester_session_key == requester_key)
            .cloned()
            .collect()
    }

    /// List all active runs
    pub async fn list_all(&self) -> Vec<SubagentRun> {
        let runs = self.runs.read().await;
        runs.values().cloned().collect()
    }

    /// Get count of active runs
    pub async fn count(&self) -> usize {
        let runs = self.runs.read().await;
        runs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run(run_id: &str, requester: &str) -> SubagentRun {
        SubagentRun {
            run_id: run_id.to_string(),
            child_session_key: format!("agent:main:subagent:{}", run_id),
            requester_session_key: requester.to_string(),
            task: "Test task".to_string(),
            cleanup: "keep".to_string(),
            label: None,
            started_at: 0,
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = SubagentRegistry::new();
        let run = test_run("run1", "agent:main:main");

        registry.register(run.clone()).await;

        let retrieved = registry.get("run1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().run_id, "run1");
    }

    #[tokio::test]
    async fn test_remove() {
        let registry = SubagentRegistry::new();
        registry.register(test_run("run1", "agent:main:main")).await;

        let removed = registry.remove("run1").await;
        assert!(removed.is_some());

        let retrieved = registry.get("run1").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_list_by_requester() {
        let registry = SubagentRegistry::new();
        registry.register(test_run("run1", "agent:main:main")).await;
        registry.register(test_run("run2", "agent:main:main")).await;
        registry.register(test_run("run3", "agent:work:main")).await;

        let main_runs = registry.list_by_requester("agent:main:main").await;
        assert_eq!(main_runs.len(), 2);

        let work_runs = registry.list_by_requester("agent:work:main").await;
        assert_eq!(work_runs.len(), 1);
    }

    #[tokio::test]
    async fn test_count() {
        let registry = SubagentRegistry::new();
        assert_eq!(registry.count().await, 0);

        registry.register(test_run("run1", "agent:main:main")).await;
        assert_eq!(registry.count().await, 1);

        registry.register(test_run("run2", "agent:main:main")).await;
        assert_eq!(registry.count().await, 2);
    }
}

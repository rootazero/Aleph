//! BackgroundAgentTracker — tracks sub-agents running in background tokio tasks.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::sync_primitives::RwLock;
use tokio_util::sync::CancellationToken;

pub struct BackgroundAgentTracker {
    running: RwLock<HashMap<String, RunningAgent>>,
    completed: RwLock<HashMap<String, CompletedAgent>>,
}

struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
}

struct CompletedAgent {
    result: Result<String, String>,
    completed_at: Instant,
}

impl BackgroundAgentTracker {
    pub fn new() -> Self {
        Self {
            running: RwLock::new(HashMap::new()),
            completed: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new background agent.
    pub fn register(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
    ) {
        let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
        running.insert(
            request_id,
            RunningAgent {
                cancel_token,
                task_description,
                started_at: Instant::now(),
            },
        );
    }

    /// Mark a background agent as completed and store its result.
    pub fn mark_completed(&self, request_id: &str, result: Result<String, String>) {
        {
            let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
            running.remove(request_id);
        }
        {
            let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
            completed.insert(
                request_id.to_string(),
                CompletedAgent {
                    result,
                    completed_at: Instant::now(),
                },
            );
        }
    }

    /// Cancel a running background agent.
    pub fn cancel(&self, request_id: &str) {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        if let Some(agent) = running.get(request_id) {
            agent.cancel_token.cancel();
        }
    }

    /// Take (consume) a completed result. Returns None if not found.
    pub fn take_result(&self, request_id: &str) -> Option<Result<String, String>> {
        let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
        completed.remove(request_id).map(|c| c.result)
    }

    /// List running agents as (request_id, task_description, elapsed_secs).
    pub fn list_running(&self) -> Vec<(String, String, u64)> {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        running
            .iter()
            .map(|(id, agent)| {
                (
                    id.clone(),
                    agent.task_description.clone(),
                    agent.started_at.elapsed().as_secs(),
                )
            })
            .collect()
    }

    /// Remove completed entries older than `ttl`.
    pub fn cleanup(&self, ttl: Duration) {
        let mut completed = self.completed.write().unwrap_or_else(|e| e.into_inner());
        completed.retain(|_, agent| agent.completed_at.elapsed() < ttl);
    }
}

impl Default for BackgroundAgentTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        let running = tracker.list_running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].0, "req-1");
    }

    #[test]
    fn mark_completed_moves_from_running() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        tracker.mark_completed("req-1", Ok("done".to_string()));
        assert!(tracker.list_running().is_empty());
        let result = tracker.take_result("req-1");
        assert!(result.is_some());
        assert_eq!(result.unwrap().unwrap(), "done");
    }

    #[test]
    fn cancel_cancels_token() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        tracker.cancel("req-1");
        assert!(token_clone.is_cancelled());
    }

    #[test]
    fn take_result_returns_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        assert!(tracker.take_result("unknown").is_none());
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("old", Ok("old result".to_string()));
        tracker.cleanup(std::time::Duration::ZERO);
        assert!(tracker.take_result("old").is_none());
    }
}

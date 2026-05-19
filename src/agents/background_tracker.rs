//! BackgroundAgentTracker — tracks sub-agents running in background tokio tasks.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::agents::progress::SubagentProgress;
use crate::sync_primitives::RwLock;
use tokio_util::sync::CancellationToken;

/// C1 — completed background results older than this are pruned opportunistically
/// on each new `register()`.
const BACKGROUND_RESULT_TTL: Duration = Duration::from_secs(3600);

pub struct BackgroundAgentTracker {
    running: RwLock<HashMap<String, RunningAgent>>,
    completed: RwLock<HashMap<String, CompletedAgent>>,
}

struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
    /// FIFO-capped progress events; capacity 50.
    progress: VecDeque<SubagentProgress>,
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
        // C1 — opportunistically prune stale completed results so they don't
        // accumulate unbounded when callers never poll `take_result`.
        self.cleanup(BACKGROUND_RESULT_TTL);
        let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
        running.insert(
            request_id,
            RunningAgent {
                cancel_token,
                task_description,
                started_at: Instant::now(),
                progress: VecDeque::with_capacity(50),
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

    /// Append a progress event to the running agent's queue.
    /// Capped at 50 events FIFO. Silently no-ops if request_id is unknown
    /// (race condition: tracker may have moved entry to completed).
    pub fn push_progress(&self, request_id: &str, event: SubagentProgress) {
        let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
        if let Some(agent) = running.get_mut(request_id) {
            if agent.progress.len() >= 50 {
                agent.progress.pop_front();
            }
            agent.progress.push_back(event);
        }
    }

    /// Return up to `limit` most-recent progress events (chronological order).
    /// Returns empty Vec if request_id is unknown or already completed.
    pub fn progress_snapshot(&self, request_id: &str, limit: usize) -> Vec<SubagentProgress> {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        match running.get(request_id) {
            Some(agent) => {
                let total = agent.progress.len();
                let start = total.saturating_sub(limit);
                agent.progress.iter().skip(start).cloned().collect()
            }
            None => Vec::new(),
        }
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

    #[test]
    fn register_prunes_stale_completed_keeps_fresh() {
        let tracker = BackgroundAgentTracker::new();
        // A freshly completed entry must survive a register() call.
        tracker.mark_completed("fresh", Ok("r".to_string()));
        let token = CancellationToken::new();
        tracker.register("new-run".to_string(), token, "task".to_string());
        assert!(
            tracker.take_result("fresh").is_some(),
            "register() must not evict a still-fresh completed entry"
        );
    }

    use crate::agents::progress::{ProgressKind, SubagentProgress};
    use std::time::SystemTime;

    fn fake_progress(step: usize) -> SubagentProgress {
        SubagentProgress {
            step,
            timestamp: SystemTime::now(),
            kind: ProgressKind::ToolCalled,
            tool_name: Some(format!("tool_{step}")),
            latency_ms: None,
            preview: None,
        }
    }

    #[test]
    fn tracker_push_progress_caps_at_50() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "task".into());

        for i in 0..51 {
            tracker.push_progress("rid", fake_progress(i));
        }

        let snap = tracker.progress_snapshot("rid", 100);
        assert_eq!(snap.len(), 50, "cap enforced at 50");
        assert_eq!(snap.first().unwrap().step, 1, "step 0 evicted FIFO");
        assert_eq!(snap.last().unwrap().step, 50);
    }

    #[test]
    fn tracker_progress_snapshot_returns_last_n() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "task".into());

        for i in 0..5 {
            tracker.push_progress("rid", fake_progress(i));
        }

        let snap = tracker.progress_snapshot("rid", 3);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].step, 2);
        assert_eq!(snap[2].step, 4);
    }

    #[test]
    fn tracker_push_unknown_id_no_op() {
        let tracker = BackgroundAgentTracker::new();
        tracker.push_progress("never-registered", fake_progress(0));
        assert!(tracker.progress_snapshot("never-registered", 10).is_empty());
    }
}

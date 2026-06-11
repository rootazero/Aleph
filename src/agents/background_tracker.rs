//! `BackgroundAgentTracker` — tracks sub-agents running in background tokio tasks.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::agents::progress::SubagentProgress;
use crate::sync_primitives::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::warn;

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
    outcome: CompletedOutcome,
    completed_at: Instant,
    /// Wall-clock seconds the run took (registration → completion).
    duration_secs: u64,
    /// Task description carried over from the running entry so `list` and
    /// `result_snapshot` can still name a finished agent.
    task_description: String,
}

/// Terminal outcome of a background subagent run.
///
/// `Ok` mirrors the foreground spawn path's `{result, iterations,
/// tool_calls_made}` response shape so `check_status` of a finished
/// background agent surfaces the same metrics regardless of which path
/// produced it. `Err` carries the failure message.
#[derive(Debug, Clone)]
pub enum CompletedOutcome {
    Ok {
        final_text: String,
        iterations: usize,
        tool_calls_made: usize,
        total_tokens: usize,
    },
    Err(String),
}

impl CompletedOutcome {
    /// Convenience constructor for a text-only success (tests, and callers
    /// with no run metrics to report).
    pub fn ok_text(text: impl Into<String>) -> Self {
        Self::Ok {
            final_text: text.into(),
            iterations: 0,
            tool_calls_made: 0,
            total_tokens: 0,
        }
    }
}

/// Non-destructive view of a finished background subagent.
#[derive(Debug, Clone)]
pub struct CompletedSnapshot {
    pub task: String,
    pub duration_secs: u64,
    pub outcome: CompletedOutcome,
}

/// Lightweight metadata for a still-running background subagent.
#[derive(Debug, Clone)]
pub struct RunningMeta {
    pub elapsed_secs: u64,
    pub task: String,
}

impl BackgroundAgentTracker {
    #[must_use]
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
        let mut running = self.running.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running.insert(
            request_id,
            RunningAgent {
                cancel_token,
                task_description,
                started_at: Instant::now(),
                progress: VecDeque::with_capacity(50),
            },
        );
        drop(running);
        self.cleanup(BACKGROUND_RESULT_TTL);
    }

    /// Mark a background agent as finished and store its outcome.
    ///
    /// The entry stays queryable **non-destructively** until pruned by the
    /// TTL in `cleanup`, so a parent may poll `check_status` for the same
    /// `request_id` more than once without the result vanishing.
    pub fn mark_completed(&self, request_id: &str, outcome: CompletedOutcome) {
        let now = Instant::now();
        let prior = {
            let mut running = self.running.write().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            running.remove(request_id)
        };
        let (duration_secs, task_description) = match prior {
            Some(agent) => (
                now.duration_since(agent.started_at).as_secs(),
                agent.task_description,
            ),
            None => (0, String::new()),
        };
        let mut completed = self.completed.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed.insert(
            request_id.to_string(),
            CompletedAgent {
                outcome,
                completed_at: now,
                duration_secs,
                task_description,
            },
        );
    }

    /// Cancel a running background agent. Returns `true` if the `request_id`
    /// was found in the running set and the `CancellationToken` was hit;
    /// `false` if no such running agent exists (already completed / never
    /// registered). The cooperative cancellation still relies on the
    /// running task observing the token at the next await point.
    pub fn cancel(&self, request_id: &str) -> bool {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        if let Some(agent) = running.get(request_id) {
            agent.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Non-destructively read a finished agent's outcome. Returns `None`
    /// when the `request_id` was never registered or has been TTL-pruned.
    /// Unlike a consume, repeated polls return the same snapshot — this is
    /// what lets a parent re-check a completed subagent.
    pub fn result_snapshot(&self, request_id: &str) -> Option<CompletedSnapshot> {
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed.get(request_id).map(|c| CompletedSnapshot {
            task: c.task_description.clone(),
            duration_secs: c.duration_secs,
            outcome: c.outcome.clone(),
        })
    }

    /// List running agents as (`request_id`, `task_description`, `elapsed_secs`).
    pub fn list_running(&self) -> Vec<(String, String, u64)> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
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

    /// Lightweight metadata for one still-running agent. `None` when the
    /// `request_id` is unknown (never registered, or already completed).
    pub fn running_meta(&self, request_id: &str) -> Option<RunningMeta> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running.get(request_id).map(|agent| RunningMeta {
            elapsed_secs: agent.started_at.elapsed().as_secs(),
            task: agent.task_description.clone(),
        })
    }

    /// Snapshot every finished agent as `(request_id, CompletedSnapshot)`.
    /// Backs the `list` action's view of results still retrievable by the
    /// parent. Bounded by the TTL prune in `cleanup`.
    pub fn all_completed(&self) -> Vec<(String, CompletedSnapshot)> {
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed
            .iter()
            .map(|(id, c)| {
                (
                    id.clone(),
                    CompletedSnapshot {
                        task: c.task_description.clone(),
                        duration_secs: c.duration_secs,
                        outcome: c.outcome.clone(),
                    },
                )
            })
            .collect()
    }

    /// Append a progress event to the running agent's queue.
    /// Capped at 50 events FIFO. Silently no-ops if `request_id` is unknown
    /// (race condition: tracker may have moved entry to completed).
    pub fn push_progress(&self, request_id: &str, event: SubagentProgress) {
        let mut running = self.running.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        if let Some(agent) = running.get_mut(request_id) {
            if agent.progress.len() >= 50 {
                agent.progress.pop_front();
            }
            agent.progress.push_back(event);
        }
    }

    /// Return up to `limit` most-recent progress events (chronological order).
    /// Returns empty Vec if `request_id` is unknown or already completed.
    pub fn progress_snapshot(&self, request_id: &str, limit: usize) -> Vec<SubagentProgress> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
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
        let mut completed = self.completed.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
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
        tracker.mark_completed("req-1", CompletedOutcome::ok_text("done"));
        assert!(tracker.list_running().is_empty());
        let snap = tracker
            .result_snapshot("req-1")
            .expect("completed entry present");
        match snap.outcome {
            CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "done"),
            CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
        }
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
    fn result_snapshot_returns_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        assert!(tracker.result_snapshot("unknown").is_none());
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("old", CompletedOutcome::ok_text("old result"));
        tracker.cleanup(std::time::Duration::ZERO);
        assert!(tracker.result_snapshot("old").is_none());
    }

    #[test]
    fn register_prunes_stale_completed_keeps_fresh() {
        let tracker = BackgroundAgentTracker::new();
        // A freshly completed entry must survive a register() call.
        tracker.mark_completed("fresh", CompletedOutcome::ok_text("r"));
        let token = CancellationToken::new();
        tracker.register("new-run".to_string(), token, "task".to_string());
        assert!(
            tracker.result_snapshot("fresh").is_some(),
            "register() must not evict a still-fresh completed entry"
        );
    }

    #[test]
    fn result_snapshot_is_non_destructive() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("rid", CompletedOutcome::ok_text("answer"));
        // Two consecutive polls must both succeed — a consume would make
        // the second one return None and confuse the parent agent.
        assert!(tracker.result_snapshot("rid").is_some());
        assert!(
            tracker.result_snapshot("rid").is_some(),
            "result_snapshot must not consume the completed entry"
        );
    }

    #[test]
    fn mark_completed_carries_run_metrics() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "do work".into());
        tracker.mark_completed(
            "rid",
            CompletedOutcome::Ok {
                final_text: "result".into(),
                iterations: 7,
                tool_calls_made: 3,
                total_tokens: 1234,
            },
        );
        let snap = tracker.result_snapshot("rid").expect("present");
        assert_eq!(snap.task, "do work");
        match snap.outcome {
            CompletedOutcome::Ok {
                iterations,
                tool_calls_made,
                total_tokens,
                ..
            } => {
                assert_eq!(iterations, 7);
                assert_eq!(tool_calls_made, 3);
                assert_eq!(total_tokens, 1234);
            }
            CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    #[test]
    fn running_meta_returns_task_and_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "explore repo".into());
        let meta = tracker.running_meta("rid").expect("running entry present");
        assert_eq!(meta.task, "explore repo");
        assert!(tracker.running_meta("ghost").is_none());
    }

    #[test]
    fn all_completed_enumerates_finished_agents() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("a", CompletedOutcome::ok_text("ra"));
        tracker.mark_completed("b", CompletedOutcome::Err("boom".into()));
        let mut ids: Vec<String> = tracker
            .all_completed()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
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

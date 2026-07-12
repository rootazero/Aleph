//! Janitor duties: reap zombie (worker-lost) and orphaned (crash-left) tasks
//! back to a schedulable or terminal state.

use std::collections::{HashMap, HashSet};

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
use crate::sync_primitives::Arc;

use super::select::{is_dispatcher_managed, is_stale_review, is_zombie};
use super::TeamDispatcher;

impl TeamDispatcher {
    /// Force-fail tasks that have been `InProgress` for longer than
    /// `zombie_ttl_secs` and are not running in this process.
    ///
    /// Distinct from [`reclaim_orphaned`], which loops orphans back to
    /// Pending — zombies have already exhausted their reasonable runtime, so
    /// retrying would just re-zombify. The dispatcher will broadcast
    /// `TeamTaskFailed` with a descriptive reason so the panel surfaces the
    /// state instead of leaving the task perpetually "in progress".
    ///
    /// Inspired by `ClawTeam`'s `list_zombie_agents(max_hours=2.0)`; threshold
    /// configurable via [`DispatcherConfig::zombie_ttl_secs`].
    pub(super) async fn reclaim_zombies(self: &Arc<Self>) {
        let zombie_ttl = self.config.zombie_ttl_secs;
        if zombie_ttl == 0 {
            return; // 0 = feature disabled
        }

        let in_progress = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::InProgress),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: reclaim_zombies list_tasks failed");
                return;
            }
        };
        // Run-set as a key-only set so the single-source-of-truth predicate
        // [`is_zombie`] decides — no inline copy of the checks to drift from it.
        let running: HashSet<String> = self.running.lock().await.keys().cloned().collect();
        let now = Self::now_epoch();

        for task in in_progress {
            if !is_zombie(&task, &running, now, zombie_ttl) {
                continue;
            }
            let elapsed = now.saturating_sub(task.started_at.unwrap_or(now));
            tracing::warn!(
                task_id = %task.id,
                age_secs = elapsed,
                zombie_ttl_secs = zombie_ttl,
                "dispatcher: declaring task a zombie (no progress beyond zombie_ttl)"
            );
            if let Some(holder) = &task.locked_by {
                if let Err(e) = self.coord_store.release_lock(&task.id, holder).await {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: release_lock failed during zombie reclaim");
                }
            }
            self.fail_task(
                &task,
                &format!("zombie timeout: no progress for {elapsed}s (limit {zombie_ttl}s)"),
            )
            .await;
        }
    }

    /// Reset in-progress tasks this process is not running back to `Pending`.
    ///
    /// On a fresh start nothing is in the `running` set, so every leftover
    /// `InProgress` task from a previous process is reclaimed and rescheduled.
    ///
    /// Pairs with [`Self::reclaim_zombies`] — that one runs first and catches
    /// the subset that has been `InProgress` past `zombie_ttl_secs` (those go
    /// straight to `Failed` instead of bouncing back to `Pending`).
    pub(super) async fn reclaim_orphaned(self: &Arc<Self>) {
        let in_progress = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::InProgress),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: reclaim_orphaned list_tasks failed");
                return;
            }
        };
        let running: HashMap<String, String> = self.running.lock().await.clone();

        for task in in_progress {
            if !is_dispatcher_managed(&task) {
                continue; // leave team_delegate-owned tasks alone
            }
            if running.contains_key(&task.id) {
                continue; // this process is actively running it
            }
            tracing::info!(task_id = %task.id, "dispatcher: reclaiming orphaned task");
            if let Some(holder) = &task.locked_by {
                if let Err(e) = self.coord_store.release_lock(&task.id, holder).await {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: release_lock failed during orphan reclaim");
                }
            }
            let _ = self
                .coord_store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        status: Some(CoordTaskStatus::Pending),
                        ..Default::default()
                    },
                )
                .await;
        }
    }

    /// Surface tasks silently stalled in `WaitingReview` past the TTL.
    ///
    /// The sibling janitor to [`Self::reclaim_zombies`] for the review gate. A
    /// review-gated task parks in `WaitingReview` after its run and waits for the
    /// lead to call `workflow_step_review` / `task_review`. Nothing reaps it —
    /// both reclaim passes only touch `InProgress` — so a review that never
    /// arrives stalls the task and its whole downstream DAG branch **silently**.
    /// This pass makes that visible: it warns once per task whose wait exceeds
    /// `zombie_ttl_secs` (reused as the review-stall threshold — no new knob).
    ///
    /// It deliberately takes **no** corrective action: the approve/reject verdict
    /// is the lead LLM's call (R7), so the dispatcher only emits an observable
    /// warning and never auto-completes the review. Dedup via
    /// [`TeamDispatcher::warned_stale_reviews`] keeps it to one warning per stuck
    /// task; the set is pruned back to the live `WaitingReview` ids each pass so a
    /// task that later returns to review can warn again.
    pub(super) async fn warn_stale_reviews(self: &Arc<Self>) {
        let ttl = self.config.zombie_ttl_secs;
        if ttl == 0 {
            return; // reuse the zombie-detection kill-switch
        }

        let waiting = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::WaitingReview),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: list waiting_review tasks failed");
                return;
            }
        };

        let now = Self::now_epoch();
        let mut warned = self.warned_stale_reviews.lock().await;
        let mut live: HashSet<String> = HashSet::new();

        for task in &waiting {
            if !is_dispatcher_managed(task) {
                continue; // team_delegate-owned reviews are the caller's problem
            }
            live.insert(task.id.clone());
            if !is_stale_review(task, now, ttl) {
                continue;
            }
            // Warn exactly once per stuck task until it leaves WaitingReview.
            if !warned.insert(task.id.clone()) {
                continue;
            }
            let waited = now.saturating_sub(task.started_at.unwrap_or(now));
            tracing::warn!(
                task_id = %task.id,
                waited_secs = waited,
                zombie_ttl_secs = ttl,
                "dispatcher: task stalled in waiting_review past TTL — lead review never arrived (verdict is the lead's call; not auto-resolving)"
            );
        }

        // Prune ids that have since left WaitingReview so the dedup set stays
        // bounded and a task that returns to review later can warn again.
        warned.retain(|id| live.contains(id));
    }
}

//! Janitor duties: reap zombie (worker-lost) and orphaned (crash-left) tasks
//! back to a schedulable or terminal state.

use std::collections::{HashMap, HashSet};

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
use crate::sync_primitives::Arc;

use super::select::{is_dispatcher_managed, is_zombie};
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
            Err(_) => return,
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
                let _ = self.coord_store.release_lock(&task.id, holder).await;
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
            Err(_) => return,
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
                let _ = self.coord_store.release_lock(&task.id, holder).await;
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
}

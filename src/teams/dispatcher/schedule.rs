//! Scheduling logic for the [`TeamDispatcher`](super::TeamDispatcher).
//!
//! `dispatch_once` is the single tick of the dumb loop: reconcile stale state,
//! select schedulable tasks, claim and launch them. It contains **no
//! reasoning** — task decomposition and routing are the leader LLM's job
//! (done via `task_create`); this only drives the DAG mechanically.

use std::collections::HashSet;

use tokio::sync::OwnedSemaphorePermit;

use super::handoff::build_handoff_context;
use super::runner::{execute_member_task, MemberRunStatus};
use super::TeamDispatcher;
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate, TaskRunStatus,
};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactType, NewArtifact, TaskStatus};
use crate::teams::context::InboxContextProvider;

/// Task metadata key marking a task as managed by the autonomous dispatcher.
///
/// Only tasks carrying `{"managed_by": "dispatcher"}` are scheduled or
/// reclaimed. Tasks created by the synchronous `team_delegate` tool omit it,
/// so the two execution paths never contend over the same row.
pub const MANAGED_BY_KEY: &str = "managed_by";
/// Value of [`MANAGED_BY_KEY`] for dispatcher-managed tasks.
pub const MANAGED_BY_DISPATCHER: &str = "dispatcher";

/// Returns whether `task` is owned by the autonomous dispatcher.
pub fn is_dispatcher_managed(task: &CoordTask) -> bool {
    task.metadata
        .get(MANAGED_BY_KEY)
        .and_then(|v| v.as_str())
        .map(|s| s == MANAGED_BY_DISPATCHER)
        .unwrap_or(false)
}

/// Pure predicate: should `task` be reaped as a zombie given the current
/// running set, wall-clock, and TTL? Extracted from [`TeamDispatcher::reclaim_zombies`]
/// so the decision logic can be exercised without spinning up a full
/// dispatcher.
///
/// Returns `true` only for `InProgress` dispatcher-managed tasks that this
/// process isn't running, have a recorded `started_at`, and whose elapsed
/// time exceeds `zombie_ttl_secs`. A `zombie_ttl_secs` of 0 disables
/// detection (matches the runtime fast-path in [`TeamDispatcher::reclaim_zombies`]).
pub fn is_zombie(
    task: &CoordTask,
    running: &HashSet<String>,
    now_epoch: u64,
    zombie_ttl_secs: u64,
) -> bool {
    if zombie_ttl_secs == 0 {
        return false;
    }
    if task.status != CoordTaskStatus::InProgress {
        return false;
    }
    if !is_dispatcher_managed(task) {
        return false;
    }
    if running.contains(&task.id) {
        return false;
    }
    let Some(started) = task.started_at else {
        return false;
    };
    now_epoch.saturating_sub(started) > zombie_ttl_secs
}

/// Pure scheduling filter: from `tasks`, pick those ready to run right now.
///
/// A task is schedulable when it is dispatcher-managed, has a derived status
/// of `Pending` (all dependencies satisfied), has an owner, is unlocked, and
/// is not already running in this process. Results are ordered by priority
/// (descending) then creation time (ascending), capped at `available_slots`.
pub fn select_schedulable(
    tasks: &[CoordTask],
    running: &HashSet<String>,
    available_slots: usize,
) -> Vec<CoordTask> {
    let mut candidates: Vec<&CoordTask> = tasks
        .iter()
        .filter(|t| t.status == CoordTaskStatus::Pending)
        .filter(|t| is_dispatcher_managed(t))
        .filter(|t| t.owner.is_some())
        .filter(|t| t.locked_by.is_none())
        .filter(|t| !running.contains(&t.id))
        .collect();

    candidates.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.created_at.cmp(&b.created_at))
    });

    candidates
        .into_iter()
        .take(available_slots)
        .cloned()
        .collect()
}

impl TeamDispatcher {
    /// One scheduling tick: reconcile, select, claim, launch.
    pub(crate) async fn dispatch_once(self: &Arc<Self>) {
        // 1. Release locks held longer than the TTL (crashed-runner safety net).
        if let Err(e) = self
            .coord_store
            .release_stale_locks(self.config.lock_ttl_secs)
            .await
        {
            tracing::warn!(error = %e, "dispatcher: release_stale_locks failed");
        }

        // 2a. Force-fail zombie tasks first — they've already exhausted their
        //     budget and would otherwise be looped back to Pending below.
        //     Order matters: zombie detection inspects `started_at` directly,
        //     so it must run before reclaim_orphaned resets it.
        self.reclaim_zombies().await;

        // 2b. Reclaim orphaned in-progress tasks (restart reconciliation).
        self.reclaim_orphaned().await;

        // 3. List schedulable pending tasks. `derive_status` already excludes
        //    tasks with unsatisfied dependencies (those report as Blocked).
        let available = self.semaphore.available_permits();
        if available == 0 {
            return;
        }
        let pending = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::Pending),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: list_tasks failed");
                return;
            }
        };

        let running_snapshot: HashSet<String> = self.running.lock().await.clone();
        let selected = select_schedulable(&pending, &running_snapshot, available);

        // 4. Claim + launch each selected task.
        for task in selected {
            let permit = match Arc::clone(&self.semaphore).try_acquire_owned() {
                Ok(p) => p,
                Err(_) => break, // concurrency cap reached
            };

            let owner = match &task.owner {
                Some(o) => o.clone(),
                None => continue, // unreachable: select_schedulable requires an owner
            };

            // Fail fast on an unknown owner instead of leaving the task stuck.
            if self.context.agent_registry().get(&owner).await.is_none() {
                self.fail_task(
                    &task,
                    &format!("Owner agent '{owner}' not found in registry"),
                )
                .await;
                continue;
            }

            // Atomic claim — loses harmlessly to a racing claimer.
            if let Err(e) = self.coord_store.acquire_lock(&task.id, &owner).await {
                tracing::debug!(task_id = %task.id, error = %e, "dispatcher: task already claimed");
                continue;
            }

            if let Err(e) = self
                .coord_store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        status: Some(CoordTaskStatus::InProgress),
                        ..Default::default()
                    },
                )
                .await
            {
                tracing::warn!(task_id = %task.id, error = %e, "dispatcher: mark in-progress failed");
                let _ = self.coord_store.release_lock(&task.id, &owner).await;
                continue;
            }

            self.running.lock().await.insert(task.id.clone());
            let dispatcher = Arc::clone(self);
            tokio::spawn(async move {
                dispatcher.run_task(task, owner, permit).await;
            });
        }
    }

    /// Wall-clock helper — extracted so tests can substitute a fixed value
    /// instead of `chrono::Utc::now()` if ever needed.
    fn now_epoch() -> u64 {
        chrono::Utc::now().timestamp().max(0) as u64
    }

    /// Force-fail tasks that have been `InProgress` for longer than
    /// `zombie_ttl_secs` and are not running in this process.
    ///
    /// Distinct from [`reclaim_orphaned`], which loops orphans back to
    /// Pending — zombies have already exhausted their reasonable runtime, so
    /// retrying would just re-zombify. The dispatcher will broadcast
    /// `TeamTaskFailed` with a descriptive reason so the panel surfaces the
    /// state instead of leaving the task perpetually "in progress".
    ///
    /// Inspired by ClawTeam's `list_zombie_agents(max_hours=2.0)`; threshold
    /// configurable via [`DispatcherConfig::zombie_ttl_secs`].
    async fn reclaim_zombies(self: &Arc<Self>) {
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
        let running: HashSet<String> = self.running.lock().await.clone();
        let now = Self::now_epoch();

        for task in in_progress {
            if !is_dispatcher_managed(&task) {
                continue;
            }
            if running.contains(&task.id) {
                continue; // owned by this process — let normal flow handle it
            }
            let Some(started) = task.started_at else {
                continue; // no clock to compare against
            };
            let elapsed = now.saturating_sub(started);
            if elapsed <= zombie_ttl {
                continue; // still within grace window
            }
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
                &format!(
                    "zombie timeout: no progress for {elapsed}s (limit {zombie_ttl}s)"
                ),
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
    async fn reclaim_orphaned(self: &Arc<Self>) {
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
        let running: HashSet<String> = self.running.lock().await.clone();

        for task in in_progress {
            if !is_dispatcher_managed(&task) {
                continue; // leave team_delegate-owned tasks alone
            }
            if running.contains(&task.id) {
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

    /// Run one claimed task end-to-end, then signal the next tick.
    pub(crate) async fn run_task(
        self: Arc<Self>,
        task: CoordTask,
        owner: String,
        _permit: OwnedSemaphorePermit,
    ) {
        let task_id = task.id.clone();
        let team_id = task.team_id.clone().unwrap_or_default();

        // Assemble the deterministic handoff context for the member.
        let inbox: Option<&dyn InboxContextProvider> = self
            .inbox_provider
            .as_deref()
            .map(|p| p as &dyn InboxContextProvider);
        let input = build_handoff_context(&self.coord_store, &self.team_store, inbox, &task).await;

        // Start a per-attempt run record so the drawer can show this
        // execution alongside any prior retries. Failure to record is
        // non-fatal — the task itself still runs and finalises normally.
        let run_id = self
            .coord_store
            .start_task_run(&task_id, &owner)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(task_id = %task_id, error = %e, "dispatcher: start_task_run failed; run history will be incomplete");
                String::new()
            });

        // G2 — autonomous dispatch runs members in parallel. Wrap each task
        // in its own git worktree to keep their indices from colliding.
        // Best-effort: non-git environments and any provision failure fall
        // back to the pre-G2 shared-tree behaviour with a logged warning.
        let outcome = execute_member_task(
            &self.context,
            &owner,
            &team_id,
            &task_id,
            input,
            self.config.task_timeout_secs,
            true,
        )
        .await;

        // Capture run outcome BEFORE we mutate the outer task — this row
        // survives any future retries and is the source of truth for the
        // attempt-by-attempt UI.
        let (run_status, run_summary, run_error) = match &outcome.status {
            MemberRunStatus::Completed => (
                TaskRunStatus::Completed,
                outcome.reply.clone(),
                None,
            ),
            MemberRunStatus::Failed => (TaskRunStatus::Failed, None, outcome.error.clone()),
            MemberRunStatus::Timeout => (TaskRunStatus::Timeout, None, outcome.error.clone()),
        };
        if let Err(e) = self
            .coord_store
            .finish_task_run(&run_id, run_status, run_summary, run_error)
            .await
        {
            tracing::warn!(task_id = %task_id, run_id = %run_id, error = %e, "dispatcher: finish_task_run failed");
        }

        match outcome.status {
            MemberRunStatus::Completed => {
                let reply = outcome.reply.unwrap_or_default();
                if let Err(e) = self
                    .coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Completed),
                            result: Some(reply.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: failed to persist task completion state; skipping artifact persistence");
                } else {
                    self.persist_artifact(&task_id, &owner, &task.subject, &reply)
                        .await;
                    // AlephEvent::TeamTaskCompleted broadcast happens inside
                    // CoordTaskStore::emit_task_topic — the panel-driven
                    // completion path gets the same downstream wiring without
                    // any caller-side fan-out.
                    tracing::info!(task_id = %task_id, "dispatcher: task completed");
                }
            }
            MemberRunStatus::Failed | MemberRunStatus::Timeout => {
                let err = outcome.error.unwrap_or_else(|| "unknown error".to_string());
                self.fail_task(&task, &err).await;
                tracing::warn!(task_id = %task_id, error = %err, "dispatcher: task failed");
            }
        }

        let _ = self.coord_store.release_lock(&task_id, &owner).await;
        self.running.lock().await.remove(&task_id);
        // Wake the loop so newly-unblocked dependents are picked up immediately.
        self.signal();
    }

    /// Mark a task `Failed`. The `AlephEvent::TeamTaskFailed` broadcast is
    /// emitted by [`CoordTaskStore::emit_task_topic`] inside `update_task`,
    /// so panel-driven failure (drawer "Fail" button) gets the same listener
    /// fan-out without per-caller wiring.
    async fn fail_task(&self, task: &CoordTask, error: &str) {
        if let Err(e) = self
            .coord_store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    result: Some(error.to_string()),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to persist task failure state");
        }
    }

    /// Persist the task result as a report artifact (best-effort).
    async fn persist_artifact(&self, task_id: &str, agent_id: &str, subject: &str, content: &str) {
        let Some(store) = &self.artifact_store else {
            return;
        };
        let _ = store
            .create_artifact(NewArtifact {
                task_id: task_id.to_string(),
                agent_id: agent_id.to_string(),
                artifact_type: ArtifactType::Report,
                title: format!("Task result: {subject}"),
                content: content.to_string(),
                metadata: serde_json::Value::Null,
                status: TaskStatus::Completed,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::Priority;

    fn task(
        id: &str,
        status: CoordTaskStatus,
        owner: Option<&str>,
        managed: bool,
        priority: Priority,
        created_at: u64,
    ) -> CoordTask {
        CoordTask {
            id: id.to_string(),
            team_id: Some("team-1".to_string()),
            subject: id.to_string(),
            description: String::new(),
            status,
            owner: owner.map(|s| s.to_string()),
            priority,
            result: None,
            metadata: if managed {
                serde_json::json!({ MANAGED_BY_KEY: MANAGED_BY_DISPATCHER })
            } else {
                serde_json::json!({})
            },
            dependencies: vec![],
            created_at,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn selects_only_pending_managed_owned_unlocked() {
        let running = HashSet::new();
        let tasks = vec![
            task(
                "ok",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                1,
            ),
            task(
                "blocked",
                CoordTaskStatus::Blocked,
                Some("a"),
                true,
                Priority::Normal,
                2,
            ),
            task(
                "no-owner",
                CoordTaskStatus::Pending,
                None,
                true,
                Priority::Normal,
                3,
            ),
            task(
                "unmanaged",
                CoordTaskStatus::Pending,
                Some("a"),
                false,
                Priority::Normal,
                4,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 10);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "ok");
    }

    #[test]
    fn skips_locked_and_running_tasks() {
        let mut running = HashSet::new();
        running.insert("running-task".to_string());
        let mut locked = task(
            "locked",
            CoordTaskStatus::Pending,
            Some("a"),
            true,
            Priority::Normal,
            1,
        );
        locked.locked_by = Some("a".to_string());
        let tasks = vec![
            locked,
            task(
                "running-task",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                2,
            ),
            task(
                "free",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                3,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 10);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "free");
    }

    #[test]
    fn orders_by_priority_then_creation() {
        let running = HashSet::new();
        let tasks = vec![
            task(
                "low-old",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Low,
                1,
            ),
            task(
                "crit-new",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Critical,
                9,
            ),
            task(
                "norm-old",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                2,
            ),
            task(
                "norm-new",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                5,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 10);
        let order: Vec<&str> = picked.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["crit-new", "norm-old", "norm-new", "low-old"]);
    }

    #[test]
    fn respects_available_slots_cap() {
        let running = HashSet::new();
        let tasks: Vec<CoordTask> = (0..10)
            .map(|i| {
                task(
                    &format!("t{i}"),
                    CoordTaskStatus::Pending,
                    Some("a"),
                    true,
                    Priority::Normal,
                    i,
                )
            })
            .collect();
        let picked = select_schedulable(&tasks, &running, 3);
        assert_eq!(picked.len(), 3);
    }

    // ---- Zombie reclamation ------------------------------------------------

    /// Build an InProgress dispatcher-managed task with `started_at` set.
    fn in_progress_task(id: &str, started_at: u64, managed: bool) -> CoordTask {
        let mut t = task(
            id,
            CoordTaskStatus::InProgress,
            Some("a"),
            managed,
            Priority::Normal,
            0,
        );
        t.started_at = Some(started_at);
        t
    }

    #[test]
    fn zombie_detection_disabled_when_ttl_zero() {
        let t = in_progress_task("zombie", 1000, true);
        let running: HashSet<String> = HashSet::new();
        assert!(!is_zombie(&t, &running, 1_000_000, 0));
    }

    #[test]
    fn zombie_detection_ignores_currently_running_task() {
        let t = in_progress_task("active", 1000, true);
        let mut running = HashSet::new();
        running.insert("active".to_string());
        assert!(!is_zombie(&t, &running, 1_000_000, 60));
    }

    #[test]
    fn zombie_detection_ignores_unmanaged_task() {
        // team_delegate-owned tasks are caller's responsibility, not ours.
        let t = in_progress_task("delegated", 1000, false);
        let running = HashSet::new();
        assert!(!is_zombie(&t, &running, 1_000_000, 60));
    }

    #[test]
    fn zombie_detection_ignores_task_without_started_at() {
        let mut t = in_progress_task("no_clock", 1000, true);
        t.started_at = None;
        let running = HashSet::new();
        assert!(!is_zombie(&t, &running, 1_000_000, 60));
    }

    #[test]
    fn zombie_detection_respects_grace_window() {
        let t = in_progress_task("young", 1_000_000, true);
        let running = HashSet::new();
        // started 30s ago, ttl 60s → not yet a zombie
        assert!(!is_zombie(&t, &running, 1_000_030, 60));
    }

    #[test]
    fn zombie_detection_fires_past_grace_window() {
        let t = in_progress_task("old", 1_000_000, true);
        let running = HashSet::new();
        // started 7201s ago, ttl 7200s → zombie
        assert!(is_zombie(&t, &running, 1_007_201, 7200));
    }

    #[test]
    fn zombie_detection_ignores_pending_tasks() {
        // A Pending task is never a zombie even if status is somehow stale —
        // only InProgress qualifies (Pending should never even have started_at).
        let mut t = in_progress_task("pending", 0, true);
        t.status = CoordTaskStatus::Pending;
        let running = HashSet::new();
        assert!(!is_zombie(&t, &running, 1_000_000, 60));
    }
}

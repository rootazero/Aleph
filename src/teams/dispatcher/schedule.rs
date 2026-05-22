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
use crate::agents::swarm::tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
use crate::event::{AlephEvent, GlobalBus};
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

/// Truncate a string to `max` chars on a char boundary (for event summaries).
fn summarize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
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

        // 2. Reclaim orphaned in-progress tasks (restart reconciliation).
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

    /// Reset in-progress tasks this process is not running back to `Pending`.
    ///
    /// On a fresh start nothing is in the `running` set, so every leftover
    /// `InProgress` task from a previous process is reclaimed and rescheduled.
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
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: failed to persist task completion state; skipping artifact and event broadcast");
                } else {
                    self.persist_artifact(&task_id, &owner, &task.subject, &reply)
                        .await;
                    GlobalBus::global()
                        .broadcast(
                            "team_dispatcher",
                            &task_id,
                            AlephEvent::TeamTaskCompleted {
                                team_id: team_id.clone(),
                                task_id: task_id.clone(),
                                result_summary: Some(summarize(&reply, 500)),
                            },
                        )
                        .await;
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

    /// Mark a task `Failed` and broadcast a `TeamTaskFailed` event.
    /// Only broadcasts the event when the database update succeeds.
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
            tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to persist task failure state; skipping event broadcast");
            return;
        }
        GlobalBus::global()
            .broadcast(
                "team_dispatcher",
                &task.id,
                AlephEvent::TeamTaskFailed {
                    team_id: task.team_id.clone().unwrap_or_default(),
                    task_id: task.id.clone(),
                    error: error.to_string(),
                },
            )
            .await;
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
}

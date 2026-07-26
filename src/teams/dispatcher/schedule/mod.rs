//! Scheduling logic for the [`TeamDispatcher`](super::TeamDispatcher).
//!
//! `dispatch_once` is the single tick of the dumb loop: reconcile stale state,
//! select schedulable tasks, claim and launch them. It contains **no
//! reasoning** — task decomposition and routing are the leader LLM's job
//! (done via `task_create`); this only drives the DAG mechanically.

mod failure;
mod reclaim;
mod select;
mod settle;

pub use select::{
    is_dispatcher_managed, is_zombie, select_schedulable, MANAGED_BY_DISPATCHER, MANAGED_BY_KEY,
};

use std::collections::HashMap;

use tokio::sync::OwnedSemaphorePermit;

use super::handoff::build_handoff_context;
use super::runner::{execute_member_task, MemberDispatchTarget, MemberRunStatus};
use super::TeamDispatcher;
use crate::agents::swarm::tasks::retry::is_retry_eligible;
use crate::agents::swarm::tasks::timeout::effective_timeout_secs;
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate, TaskRunStatus,
};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactType, NewArtifact, TaskStatus};
use crate::teams::context::InboxContextProvider;
use crate::teams::types::TeamMemberKind;

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

        // 2c. Surface tasks silently stalled awaiting lead review. Non-
        //     destructive: warns only, never advances the verdict (R7 — the
        //     approve/reject call is the lead LLM's, not the dispatcher's).
        self.warn_stale_reviews().await;

        // 2d. Push the terminal summary of any freshly-settled workflow run
        //     to its origin channel (R5 — the launching user hears the end of
        //     an autonomous run instead of polling `status`). Once-only via
        //     in-process claim + durable `workflow_notified` marker.
        self.notify_settled_workflow_runs().await;

        // 2e. Reset clarify tasks stranded Paused by a crash between park and
        //     delivery back to Pending so their question is re-delivered
        //     (runs before step 3, so re-delivery can happen this very tick).
        self.redeliver_stalled_clarify().await;

        // 2f. Close `running` run rows whose worker is gone (keyed on the
        //     runs table, not task status — the only pass that can see
        //     cancel-then-crash orphans on already-terminal tasks).
        self.abandon_orphaned_runs().await;

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

        // Retry-backoff gate (orthogonal to the fairness selection below): a task
        // re-dispatched after a failure carries a `retry_not_before` deadline in
        // its metadata; skip it until that deadline elapses so a transient
        // failure can clear before the next attempt. Tasks that never failed have
        // no deadline and pass through untouched.
        let now = Self::now_epoch();
        let eligible: Vec<CoordTask> = pending
            .into_iter()
            .filter(|t| is_retry_eligible(&t.metadata, now))
            .collect();

        let running_snapshot: HashMap<String, String> = self.running.lock().await.clone();
        let selected = select_schedulable(
            &eligible,
            &running_snapshot,
            available,
            self.config.max_per_owner,
        );

        // 4. Claim + launch each selected task.
        for task in selected {
            // Clarify steps are not agent runs: deliver the question to the
            // user's channel and park the task awaiting their reply. They take
            // no worker slot and skip owner resolution (the owner is a sentinel,
            // not a team member). See [`super::clarify`].
            if crate::workflow::clarify::is_clarify_task(&task) {
                self.handle_clarify_task(&task).await;
                continue;
            }

            let permit = match Arc::clone(&self.semaphore).try_acquire_owned() {
                Ok(p) => p,
                Err(_) => break, // concurrency cap reached
            };

            let owner = match &task.owner {
                Some(o) => o.clone(),
                None => continue, // unreachable: select_schedulable requires an owner
            };

            // Resolve the owner against the team's member roster so we
            // know whether to route in-process (Agent) or via ACP. A
            // task with no team_id or an unknown owner is a configuration
            // error and force-fails the task — never silently stalls.
            let dispatch_target = match self.resolve_dispatch_target(&task, &owner).await {
                Ok(t) => t,
                Err(reason) => {
                    self.fail_task(&task, &reason).await;
                    continue;
                }
            };

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
                if let Err(e) = self.coord_store.release_lock(&task.id, &owner).await {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: release_lock failed after mark-in-progress failure");
                }
                continue;
            }

            self.running
                .lock()
                .await
                .insert(task.id.clone(), owner.clone());
            let dispatcher = Arc::clone(self);
            tokio::spawn(async move {
                dispatcher
                    .run_task(task, owner, dispatch_target, permit)
                    .await;
            });
        }
    }

    /// Resolve a task's `owner` string to a [`MemberDispatchTarget`].
    ///
    /// - For an `Agent` kind we re-check the registry so missing in-process
    ///   agents still fail fast (preserving the pre-A2 behaviour).
    /// - For an `AcpSession` kind we trust the `team_members` row and skip
    ///   the registry check entirely — the ACP runner validates the
    ///   harness id when it tries to spawn.
    /// - Missing team or member rows surface as a fail-task reason so the
    ///   task does not loop forever.
    async fn resolve_dispatch_target(
        &self,
        task: &CoordTask,
        owner: &str,
    ) -> Result<MemberDispatchTarget, String> {
        let team_id = task
            .team_id
            .as_deref()
            .ok_or_else(|| format!("task '{}' has no team_id", task.id))?;

        let members = self
            .team_store
            .get_members(team_id)
            .await
            .map_err(|e| format!("team_store.get_members failed: {e}"))?;
        let member = members
            .iter()
            .find(|m| m.agent_id == owner)
            .ok_or_else(|| format!("owner '{owner}' is not a member of team '{team_id}'"))?;

        match member.kind {
            TeamMemberKind::Agent => {
                if self.context.agent_registry().get(owner).await.is_none() {
                    return Err(format!("Owner agent '{owner}' not found in registry"));
                }
                Ok(MemberDispatchTarget::Agent {
                    agent_id: owner.to_string(),
                })
            }
            TeamMemberKind::AcpSession => {
                MemberDispatchTarget::from_member(member).ok_or_else(|| {
                    format!(
                        "ACP member '{owner}' is missing required routing fields (harness_id/cwd)"
                    )
                })
            }
        }
    }

    /// Wall-clock helper — extracted so tests can substitute a fixed value
    /// instead of `chrono::Utc::now()` if ever needed.
    pub(super) fn now_epoch() -> u64 {
        chrono::Utc::now().timestamp().max(0) as u64
    }

    /// Run one claimed task end-to-end, then signal the next tick.
    pub(crate) async fn run_task(
        self: Arc<Self>,
        task: CoordTask,
        owner: String,
        target: MemberDispatchTarget,
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

        // ACP members never get a per-task worktree — the external CLI
        // process owns its own cwd. Only in-process agents benefit from
        // git worktree isolation.
        let isolate = matches!(target, MemberDispatchTarget::Agent { .. });
        // A workflow step may pin its member run to a specific model; plain team
        // tasks carry no such key, so this is `None` and the run keeps its
        // default model.
        let model_override = crate::workflow::workflow_model_override(&task.metadata);
        // Same contract for the per-step reasoning-effort override: stamped by
        // `workflow::materialize`, normalised here, delivered to the member run
        // as its `think_level` metadata. Absent → session default depth.
        let think_level = crate::workflow::workflow_effort_think_level(&task.metadata)
            .map(|l| l.id().to_string());
        // A task may override the global single-attempt timeout via its metadata
        // (`timeout_secs`), so a deep research subtask and a quick formatting step
        // no longer share one budget. Absent → the configured global default.
        let timeout_secs = effective_timeout_secs(&task.metadata, self.config.task_timeout_secs);

        // Tree budget (autonomous dispatch): if the task carries an
        // `origin_session` anchor (stamped by the creating tool while it had a
        // live turn context) and this is an in-process member (ACP runs accrue
        // no SessionStore tokens), enroll the child run into the creating
        // session's goal tree budget. Account-ONLY — there is no model turn here
        // to receive a refusal, so the leader's own pursuit budget still stops
        // the tree at its continuation hook; this only makes the autonomous
        // child's spend visible to that accounting. Best-effort.
        if isolate {
            if let Some(origin) =
                crate::gateway::goal_budget::origin_session_from_metadata(&task.metadata)
            {
                let child_key = crate::gateway::router::SessionKey::task(
                    &owner,
                    crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
                    &task_id,
                );
                let _ = crate::gateway::goal_budget::check_and_enroll_delegation(
                    self.context.session_store(),
                    &origin,
                    &child_key,
                    false,
                )
                .await;
            }
        }

        let outcome = execute_member_task(
            &self.context,
            &target,
            &team_id,
            &task_id,
            input,
            uuid::Uuid::new_v4().to_string(),
            timeout_secs,
            isolate,
            model_override,
            think_level,
        )
        .await;

        // Capture run outcome BEFORE we mutate the outer task — this row
        // survives any future retries and is the source of truth for the
        // attempt-by-attempt UI.
        let (run_status, run_summary, run_error) = match &outcome.status {
            MemberRunStatus::Completed => (TaskRunStatus::Completed, outcome.reply.clone(), None),
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

        // Finalize fence: `task` is the snapshot claimed at dispatch time, so
        // anything that changed the row while the member ran (a cancel, an
        // operator hard-retry back to Pending, or — after a stale-lock release
        // on a long run — a whole successor claim) would otherwise be silently
        // overwritten here. The attempt itself is already recorded in
        // coord_task_runs above; [`select::finalize_disposition`] decides
        // whether the terminal write (and whose lock/running entry) is still
        // ours. Re-fetch failures degrade to `Proceed` (P7 — the pre-fence
        // behaviour).
        let latest_run_id: Option<String> = if run_id.is_empty() {
            None
        } else {
            match self.coord_store.list_task_runs(&task_id).await {
                Ok(runs) => runs.last().map(|r| r.id.clone()),
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: run-history re-read failed; finalize fence degraded");
                    None
                }
            }
        };
        let current_status = match self.coord_store.get_task(&task_id).await {
            Ok(Some(t)) => Some(t.status),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(task_id = %task_id, error = %e, "dispatcher: task re-fetch failed; finalize fence degraded");
                None
            }
        };
        let disposition =
            select::finalize_disposition(&run_id, latest_run_id.as_deref(), current_status);
        match &disposition {
            select::RunFinalize::Proceed => {}
            select::RunFinalize::KeepForeignState(s) => {
                tracing::info!(task_id = %task_id, status = %s, "dispatcher: task left in_progress mid-flight; keeping the newer state");
            }
            select::RunFinalize::Superseded => {
                tracing::warn!(task_id = %task_id, run_id = %run_id, "dispatcher: run superseded by a newer claim; leaving task/lock/tracking to the successor");
            }
        }

        match outcome.status {
            _ if disposition != select::RunFinalize::Proceed => {}
            MemberRunStatus::Completed => {
                let reply = outcome.reply.unwrap_or_default();
                // Review-gated tasks park in WaitingReview for the lead to
                // resolve via workflow_step_review; dependents stay blocked
                // until the verdict. Everything else completes directly.
                let final_status = select::completion_status(&task);
                if let Err(e) = self
                    .coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(final_status),
                            result: Some(reply.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: failed to persist task completion state; skipping artifact persistence");
                } else {
                    // The work product exists regardless of the verdict, so
                    // the artifact persists on both paths — reviewers read it.
                    self.persist_artifact(&task_id, &owner, &task.subject, &reply)
                        .await;
                    // AlephEvent::TeamTaskCompleted (or TeamTaskUpdated with
                    // status "waiting_review") broadcast happens inside
                    // CoordTaskStore::emit_task_topic — the panel-driven
                    // completion path gets the same downstream wiring without
                    // any caller-side fan-out.
                    if final_status == CoordTaskStatus::WaitingReview {
                        tracing::info!(task_id = %task_id, "dispatcher: task awaiting lead review");
                    } else {
                        tracing::info!(task_id = %task_id, "dispatcher: task completed");
                    }
                }
            }
            MemberRunStatus::Failed | MemberRunStatus::Timeout => {
                let err = outcome.error.unwrap_or_else(|| "unknown error".to_string());
                // Bounded automatic retry: a transient attempt failure is
                // re-dispatched (resuming with recovery context) until the
                // task's retry ceiling is hit, then it becomes FailedFinal.
                self.fail_or_retry(&task, &err).await;
                tracing::warn!(task_id = %task_id, error = %err, "dispatcher: task attempt failed");
            }
        }

        // Teardown — but only of what is still OURS. A superseding claim owns
        // the lock (same owner string would match!) and the running-map entry
        // (same task-id key); releasing/evicting them here would let a third
        // claim start while the successor still runs.
        if disposition != select::RunFinalize::Superseded {
            if let Err(e) = self.coord_store.release_lock(&task_id, &owner).await {
                tracing::warn!(task_id = %task_id, error = %e, "dispatcher: release_lock failed at run_task teardown");
            }
            self.running.lock().await.remove(&task_id);
        }
        // Wake the loop so newly-unblocked dependents are picked up immediately.
        self.signal();
    }

    /// Persist the task result as a report artifact (best-effort).
    async fn persist_artifact(&self, task_id: &str, agent_id: &str, subject: &str, content: &str) {
        let Some(store) = &self.artifact_store else {
            return;
        };
        if let Err(e) = store
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
            .await
        {
            tracing::warn!(task_id = %task_id, error = %e, "dispatcher: persist_artifact create_artifact failed");
        }
    }
}

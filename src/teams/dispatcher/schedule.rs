//! Scheduling logic for the [`TeamDispatcher`](super::TeamDispatcher).
//!
//! `dispatch_once` is the single tick of the dumb loop: reconcile stale state,
//! select schedulable tasks, claim and launch them. It contains **no
//! reasoning** — task decomposition and routing are the leader LLM's job
//! (done via `task_create`); this only drives the DAG mechanically.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;

use super::handoff::build_handoff_context;
use super::runner::{execute_member_task, MemberDispatchTarget, MemberRunStatus};
use super::TeamDispatcher;
use crate::agents::swarm::tasks::acceptance::lead_review_required;
use crate::agents::swarm::tasks::retry::{
    is_retry_eligible, jittered_backoff_secs, read_max_retries, retry_decision,
    with_retry_not_before, RetryDecision,
};
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate, TaskRunStatus,
};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactType, NewArtifact, TaskStatus};
use crate::teams::context::InboxContextProvider;
use crate::teams::types::TeamMemberKind;

/// Task metadata key marking a task as managed by the autonomous dispatcher.
///
/// Only tasks carrying `{"managed_by": "dispatcher"}` are scheduled or
/// reclaimed. Tasks created by the synchronous `team_delegate` tool omit it,
/// so the two execution paths never contend over the same row.
pub const MANAGED_BY_KEY: &str = "managed_by";
/// Value of [`MANAGED_BY_KEY`] for dispatcher-managed tasks.
pub const MANAGED_BY_DISPATCHER: &str = "dispatcher";

/// Returns whether `task` is owned by the autonomous dispatcher.
#[must_use]
pub fn is_dispatcher_managed(task: &CoordTask) -> bool {
    task.metadata
        .get(MANAGED_BY_KEY)
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == MANAGED_BY_DISPATCHER)
}

/// Terminal status for a successful member run: review-gated tasks
/// (`lead_review_required` in metadata, stamped by the workflow compiler)
/// park in `WaitingReview` for `workflow_step_review` to resolve; everything
/// else completes directly. Pure — the review verdict itself is the lead
/// LLM's call (R7), this only routes the row.
fn completion_status(task: &CoordTask) -> CoordTaskStatus {
    if lead_review_required(&task.metadata) {
        CoordTaskStatus::WaitingReview
    } else {
        CoordTaskStatus::Completed
    }
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
#[must_use]
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

/// Pure scheduling filter: from `tasks`, pick those ready to run right now,
/// fairly distributed across owners.
///
/// A task is schedulable when it is dispatcher-managed, has a derived status
/// of `Pending` (all dependencies satisfied), has an owner, is unlocked, and
/// is not already running in this process. The candidate pool is ordered by
/// priority (descending) then creation time (ascending) — preserving the
/// leader-set priority intent — and then filled with **load-balanced
/// round-robin across owners**: each slot goes to the eligible task whose
/// owner is currently least busy (counting both this-process in-flight tasks,
/// supplied via `running`, and earlier picks in this same pass), with the
/// priority/FIFO order as the tiebreaker.
///
/// This is the multi-agent fairness guarantee — one greedy owner can no longer
/// monopolise the whole concurrency pool while other team members have ready
/// work, mirroring openclaw's per-lane `maxConcurrent` and hermes' per-source
/// limit. `max_per_owner` (`0` = disabled) additionally enforces a hard cap on
/// how many concurrent tasks any single owner may hold.
///
/// Backward-compatible: when only one owner has ready work, or when every
/// candidate fits in `available_slots`, the result is the same set the old
/// strict-priority `take(available_slots)` produced (single-owner picks fall
/// through to pure priority/FIFO order).
#[must_use]
pub fn select_schedulable(
    tasks: &[CoordTask],
    running: &HashMap<String, String>,
    available_slots: usize,
    max_per_owner: usize,
) -> Vec<CoordTask> {
    if available_slots == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<&CoordTask> = tasks
        .iter()
        .filter(|t| t.status == CoordTaskStatus::Pending)
        .filter(|t| is_dispatcher_managed(t))
        .filter(|t| t.owner.is_some())
        .filter(|t| t.locked_by.is_none())
        .filter(|t| !running.contains_key(&t.id))
        .collect();

    // Base order: leader-set priority first, then FIFO. Within one owner this
    // is the exact consumption order; across owners it is the tiebreaker.
    candidates.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.created_at.cmp(&b.created_at))
    });

    // Per-owner concurrency load, seeded from tasks already in flight in this
    // process. Owned `String` keys keep the fill free of borrow gymnastics —
    // owner ids are short and the candidate set per tick is small.
    let mut load: HashMap<String, usize> = HashMap::new();
    for owner in running.values() {
        *load.entry(owner.clone()).or_insert(0) += 1;
    }

    let mut picked: Vec<CoordTask> = Vec::with_capacity(available_slots);
    let mut consumed = vec![false; candidates.len()];

    while picked.len() < available_slots {
        // Pick the eligible candidate whose owner is least loaded; ties resolve
        // to the earliest in priority/FIFO order (first un-consumed wins).
        let mut best: Option<usize> = None;
        let mut best_load = usize::MAX;
        for (i, task) in candidates.iter().enumerate() {
            if consumed[i] {
                continue;
            }
            let owner = task.owner.as_deref().unwrap_or("");
            let owner_load = *load.get(owner).unwrap_or(&0);
            if max_per_owner > 0 && owner_load >= max_per_owner {
                continue; // hard per-owner cap reached
            }
            if owner_load < best_load {
                best_load = owner_load;
                best = Some(i);
            }
        }
        let Some(i) = best else {
            break; // nothing left that is both un-consumed and under cap
        };
        consumed[i] = true;
        let owner = candidates[i].owner.clone().unwrap_or_default();
        *load.entry(owner).or_insert(0) += 1;
        picked.push(candidates[i].clone());
    }

    picked
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
                let _ = self.coord_store.release_lock(&task.id, &owner).await;
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
    /// Inspired by `ClawTeam`'s `list_zombie_agents(max_hours=2.0)`; threshold
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
        let running: HashMap<String, String> = self.running.lock().await.clone();
        let now = Self::now_epoch();

        for task in in_progress {
            if !is_dispatcher_managed(&task) {
                continue;
            }
            if running.contains_key(&task.id) {
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
        let outcome = execute_member_task(
            &self.context,
            &target,
            &team_id,
            &task_id,
            input,
            self.config.task_timeout_secs,
            isolate,
            model_override,
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

        // Cancelled-while-in-flight guard: `task` is the snapshot claimed at
        // dispatch time, so a cancel issued during the member run (workflow
        // `cancel` / `team_task_control.cancel`) would otherwise be silently
        // overwritten here and resurrect the task. The attempt itself is
        // already recorded in coord_task_runs above; only the task's terminal
        // status is preserved. A re-fetch failure proceeds normally (P7
        // graceful degradation — same behaviour as before the guard).
        let cancelled_mid_flight = matches!(
            self.coord_store.get_task(&task_id).await,
            Ok(Some(t)) if t.status == CoordTaskStatus::Cancelled
        );
        if cancelled_mid_flight {
            tracing::info!(task_id = %task_id, "dispatcher: task was cancelled mid-flight; keeping it cancelled");
        }

        match outcome.status {
            _ if cancelled_mid_flight => {}
            MemberRunStatus::Completed => {
                let reply = outcome.reply.unwrap_or_default();
                // Review-gated tasks park in WaitingReview for the lead to
                // resolve via workflow_step_review; dependents stay blocked
                // until the verdict. Everything else completes directly.
                let final_status = completion_status(&task);
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

        let _ = self.coord_store.release_lock(&task_id, &owner).await;
        self.running.lock().await.remove(&task_id);
        // Wake the loop so newly-unblocked dependents are picked up immediately.
        self.signal();
    }

    /// Decide a failed/timed-out attempt's fate: **bounded automatic retry**, or
    /// a terminal `Failed` (the doc's `FailedFinal`).
    ///
    /// Counts the failed/timed-out attempts already recorded for the task — the
    /// just-failed attempt is recorded (`finish_task_run`) *before* this runs —
    /// against the task's retry ceiling (`max_retries` in metadata, else the
    /// dispatcher's [`default_max_retries`](super::DispatcherConfig::default_max_retries)).
    ///
    /// - **Under the ceiling** → reset to `Pending`; the next tick re-claims it
    ///   and [`build_handoff_context`](super::handoff::build_handoff_context)
    ///   surfaces the prior attempts as recovery context, so the resuming member
    ///   resumes instead of cold-starting. The retry *count* is the only
    ///   mechanical decision here; *how* to recover is the model's call via the
    ///   injected prompt (R7/R9/R10).
    /// - **At/over the ceiling** → [`fail_task`](Self::fail_task) → terminal
    ///   `Failed`.
    ///
    /// Orphan reclaims leave a `Running` row that never finished, so they do not
    /// consume the retry budget — only clean `Failed`/`Timeout` attempts do.
    /// Zombies bypass this entirely and go straight to `fail_task` (they have
    /// already exhausted their runtime budget; retrying would just re-zombify).
    ///
    /// Cancelled stays sticky on both paths — a cancel issued mid-flight is
    /// neither retried nor overwritten with a failure.
    pub(super) async fn fail_or_retry(&self, task: &CoordTask, error: &str) {
        // Cancelled-sticky guard for the retry path (fail_task re-checks for the
        // give-up path); never resurrect a task cancelled since the snapshot.
        if matches!(
            self.coord_store.get_task(&task.id).await,
            Ok(Some(t)) if t.status == CoordTaskStatus::Cancelled
        ) {
            tracing::info!(task_id = %task.id, "dispatcher: task cancelled; neither retrying nor failing");
            return;
        }

        let max_retries =
            read_max_retries(&task.metadata).unwrap_or(self.config.default_max_retries);
        let failed_attempts = self
            .coord_store
            .list_task_runs(&task.id)
            .await
            .map(|runs| {
                runs.iter()
                    .filter(|r| matches!(r.status, TaskRunStatus::Failed | TaskRunStatus::Timeout))
                    .count() as u32
            })
            .unwrap_or(0);

        match retry_decision(failed_attempts, max_retries) {
            RetryDecision::Retry => {
                // Exponential backoff (with jitter): stamp the earliest epoch
                // this task may be re-claimed into metadata, so the scheduler's
                // eligibility gate spaces the next attempt out instead of
                // re-claiming on the next tick. `0` backoff → immediate (the
                // pre-enhancement behaviour). The jitter seed is a hash of the
                // task id — deterministic and RNG-free, but distinct per task so
                // a team whose tasks failed together don't all retry in lockstep
                // and re-stampede the recovering provider (thundering herd).
                let seed = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    task.id.hash(&mut h);
                    h.finish()
                };
                let backoff = jittered_backoff_secs(
                    failed_attempts,
                    self.config.retry_backoff_base_secs,
                    self.config.retry_backoff_cap_secs,
                    seed,
                );
                let not_before = Self::now_epoch().saturating_add(backoff);
                let metadata = with_retry_not_before(task.metadata.clone(), not_before);
                tracing::info!(
                    task_id = %task.id,
                    attempt = failed_attempts,
                    max_retries,
                    backoff_secs = backoff,
                    "dispatcher: task attempt failed; scheduling retry"
                );
                // Reset to Pending so a later tick re-dispatches. Surfacing the
                // last error as the (transient) result keeps the panel honest
                // until the retry overwrites it; the durable recovery context is
                // the run log + exit journal, assembled at hand-off time.
                if let Err(e) = self
                    .coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(format!(
                                "retry {failed_attempts}/{max_retries} in {backoff}s after: {error}"
                            )),
                            metadata: Some(metadata),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to reset task for retry; marking failed");
                    self.fail_task(task, error).await;
                } else if backoff > 0 {
                    // Precise wake so a short backoff isn't stranded until the
                    // (minute-scale) fallback tick. A detached timer that fires a
                    // single signal is cheap and idiomatic — leaning on Tokio
                    // rather than busy-polling the deadline.
                    let signal = Arc::clone(&self.signal);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        signal.notify_one();
                    });
                }
            }
            RetryDecision::GiveUp => self.fail_task(task, error).await,
        }
    }

    /// Mark a task terminally `Failed` (`FailedFinal`). The
    /// `AlephEvent::TeamTaskFailed` broadcast is emitted by
    /// [`CoordTaskStore::emit_task_topic`] inside `update_task`, so panel-driven
    /// failure (drawer "Fail" button) gets the same listener fan-out without
    /// per-caller wiring.
    ///
    /// `pub(super)` so the clarify executor ([`super::clarify`]) can terminate
    /// an unanswerable clarification with the same path. Reached from
    /// [`fail_or_retry`](Self::fail_or_retry) once the retry budget is spent, and
    /// directly from zombie reclamation (which never retries).
    ///
    /// Cancelled is sticky: if the task was cancelled since the caller's
    /// snapshot was taken, the failure is NOT written over it — the attempt
    /// is already recorded in run history and a cancelled task must stay
    /// cancelled.
    pub(super) async fn fail_task(&self, task: &CoordTask, error: &str) {
        if matches!(
            self.coord_store.get_task(&task.id).await,
            Ok(Some(t)) if t.status == CoordTaskStatus::Cancelled
        ) {
            tracing::info!(task_id = %task.id, "dispatcher: not overwriting cancelled task with failure");
            return;
        }
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
    fn completion_status_routes_review_gated_tasks_to_waiting_review() {
        use crate::agents::swarm::tasks::acceptance::with_lead_review_required;

        let plain = task(
            "t1",
            CoordTaskStatus::InProgress,
            Some("a"),
            true,
            Priority::Normal,
            1,
        );
        assert_eq!(completion_status(&plain), CoordTaskStatus::Completed);

        let mut gated = task(
            "t2",
            CoordTaskStatus::InProgress,
            Some("a"),
            true,
            Priority::Normal,
            1,
        );
        gated.metadata = with_lead_review_required(gated.metadata, true);
        assert_eq!(completion_status(&gated), CoordTaskStatus::WaitingReview);
    }

    #[test]
    fn selects_only_pending_managed_owned_unlocked() {
        let running = HashMap::new();
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
        let picked = select_schedulable(&tasks, &running, 10, 0);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "ok");
    }

    #[test]
    fn skips_locked_and_running_tasks() {
        let mut running = HashMap::new();
        running.insert("running-task".to_string(), "a".to_string());
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
        let picked = select_schedulable(&tasks, &running, 10, 0);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "free");
    }

    #[test]
    fn orders_by_priority_then_creation() {
        let running = HashMap::new();
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
        let picked = select_schedulable(&tasks, &running, 10, 0);
        let order: Vec<&str> = picked.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["crit-new", "norm-old", "norm-new", "low-old"]);
    }

    #[test]
    fn respects_available_slots_cap() {
        let running = HashMap::new();
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
        let picked = select_schedulable(&tasks, &running, 3, 0);
        assert_eq!(picked.len(), 3);
    }

    // ---- Multi-agent fairness ---------------------------------------------

    #[test]
    fn spreads_slots_across_owners_round_robin() {
        // Three owners each with two ready tasks; only three slots free. A
        // strict-priority `take(3)` could hand all slots to one owner; the
        // fair scheduler must give one slot to each distinct owner.
        let running = HashMap::new();
        let tasks = vec![
            task(
                "a1",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                1,
            ),
            task(
                "b1",
                CoordTaskStatus::Pending,
                Some("b"),
                true,
                Priority::Normal,
                2,
            ),
            task(
                "c1",
                CoordTaskStatus::Pending,
                Some("c"),
                true,
                Priority::Normal,
                3,
            ),
            task(
                "a2",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                4,
            ),
            task(
                "b2",
                CoordTaskStatus::Pending,
                Some("b"),
                true,
                Priority::Normal,
                5,
            ),
            task(
                "c2",
                CoordTaskStatus::Pending,
                Some("c"),
                true,
                Priority::Normal,
                6,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 3, 0);
        assert_eq!(picked.len(), 3);
        let owners: HashSet<&str> = picked.iter().filter_map(|t| t.owner.as_deref()).collect();
        assert_eq!(owners.len(), 3, "each owner should get exactly one slot");
        // Within the fair fill, the priority/FIFO tiebreaker still picks each
        // owner's first task.
        let ids: HashSet<&str> = picked.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, HashSet::from(["a1", "b1", "c1"]));
    }

    #[test]
    fn inflight_load_defers_busy_owner() {
        // Owner "a" is already running one task; with a single free slot the
        // idle owner "b" wins it even though "a"'s task is earlier in FIFO.
        let mut running = HashMap::new();
        running.insert("a-running".to_string(), "a".to_string());
        let tasks = vec![
            task(
                "a1",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                1,
            ),
            task(
                "b1",
                CoordTaskStatus::Pending,
                Some("b"),
                true,
                Priority::Normal,
                2,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 1, 0);
        assert_eq!(picked.len(), 1);
        assert_eq!(
            picked[0].id, "b1",
            "freed slot should go to the least-busy owner"
        );
    }

    #[test]
    fn max_per_owner_caps_single_owner() {
        // A single owner with five ready tasks and four free slots is capped
        // at two concurrent tasks by `max_per_owner = 2`.
        let running = HashMap::new();
        let tasks: Vec<CoordTask> = (0..5)
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
        let picked = select_schedulable(&tasks, &running, 4, 2);
        assert_eq!(
            picked.len(),
            2,
            "hard per-owner cap bounds one owner below the slot count"
        );
    }

    #[test]
    fn fair_fill_is_byte_identical_for_single_owner() {
        // The fairness path must not perturb the single-owner case: same set,
        // same priority/FIFO order as the old strict-priority `take`.
        let running = HashMap::new();
        let tasks = vec![
            task(
                "low",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Low,
                3,
            ),
            task(
                "crit",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Critical,
                1,
            ),
            task(
                "norm",
                CoordTaskStatus::Pending,
                Some("a"),
                true,
                Priority::Normal,
                2,
            ),
        ];
        let picked = select_schedulable(&tasks, &running, 2, 0);
        let order: Vec<&str> = picked.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, vec!["crit", "norm"]);
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

//! Pure scheduling policy for the team dispatcher: which tasks are managed,
//! which are schedulable, and which are zombies. No I/O — exercisable without
//! a live dispatcher.

use std::collections::{HashMap, HashSet};

use crate::agents::swarm::tasks::acceptance::lead_review_required;
use crate::agents::swarm::tasks::timeout::read_task_timeout;
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus};

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
pub(super) fn completion_status(task: &CoordTask) -> CoordTaskStatus {
    if lead_review_required(&task.metadata) {
        CoordTaskStatus::WaitingReview
    } else {
        CoordTaskStatus::Completed
    }
}

/// Where an orphaned (crash-left) `InProgress` task should be put back.
///
/// `Pending` — re-dispatch — for everything, **except** a step the user paused
/// while it was still running. `workflow(action='pause')` cannot stop a live
/// member run, so it records the intent as
/// [`PAUSED_FROM_IN_PROGRESS`](crate::agents::swarm::tasks::PAUSED_FROM_IN_PROGRESS)
/// and leaves the status alone; without this branch the next boot's orphan
/// reclaim reset that row to `Pending` and the dispatcher re-ran the very step
/// the user had paused — silently, with the workflow still reporting itself
/// paused.
///
/// Parking it `Paused` hands the row to `workflow(action='resume')`, which is
/// also the only thing that returns it to janitor visibility: `Paused` is
/// invisible to both `reclaim_zombies` and `abandon_orphaned_runs` (both are
/// `InProgress`-scoped), so the stamp MUST be cleared on resume or the step
/// loses its watchdog permanently. Both resume faces clear it in the same
/// write that restores the status.
///
/// Pure, like every other decision in this module — exercisable without a live
/// dispatcher.
#[must_use]
pub fn orphan_reset_status(task: &CoordTask) -> CoordTaskStatus {
    match crate::agents::swarm::tasks::paused_from(&task.metadata) {
        Some(crate::agents::swarm::tasks::PAUSED_FROM_IN_PROGRESS) => CoordTaskStatus::Paused,
        _ => CoordTaskStatus::Pending,
    }
}

/// Pure predicate: should `task` be reaped as a zombie given the current
/// running set, wall-clock, and TTL? Extracted from [`TeamDispatcher::reclaim_zombies`]
/// so the decision logic can be exercised without spinning up a full
/// dispatcher.
///
/// Returns `true` only for `InProgress` dispatcher-managed tasks that this
/// process isn't running, have a recorded `started_at`, and whose elapsed
/// time exceeds the task's **effective** zombie threshold. A `zombie_ttl_secs`
/// of 0 disables detection (matches the runtime fast-path in
/// [`TeamDispatcher::reclaim_zombies`]).
///
/// The effective threshold is `max(zombie_ttl_secs, per-task timeout)`. The
/// global config already guarantees `zombie_ttl_secs >= task_timeout_secs` so a
/// healthy long-running task is never reaped before its own deadline; a per-task
/// timeout override ([`read_task_timeout`]) can exceed that global, so it is
/// folded in here too — otherwise a legitimately long task orphaned by a process
/// restart would be force-failed before its own budget elapsed.
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
    let effective_ttl = zombie_ttl_secs.max(read_task_timeout(&task.metadata).unwrap_or(0));
    now_epoch.saturating_sub(started) > effective_ttl
}

/// Pure predicate: has `task` been parked in `WaitingReview` longer than the
/// TTL without a lead verdict? The sibling of [`is_zombie`] for the review gate.
///
/// A review-gated task ([`completion_status`] → `WaitingReview`) finishes its
/// run and then waits for `workflow_step_review` / `task_review`. Unlike an
/// `InProgress` task, **no janitor pass reaps it** — so if the lead never
/// reviews (its run ended, or it forgot), the task and its whole downstream DAG
/// branch stall silently. This predicate lets [`TeamDispatcher::warn_stale_reviews`]
/// surface that stall. It deliberately does **not** resolve the review: the
/// approve/reject verdict is the lead LLM's call (R7), so the dispatcher only
/// makes the wait visible, never auto-completes it.
///
/// Staleness is anchored to `started_at` (the run start), a slight over-estimate
/// of the actual review wait — so the warning fires no earlier than `ttl_secs`
/// after the run began, which is the right side to err on for a coarse
/// "stuck too long" signal and needs no extra run-history query. Because of
/// that anchor the per-task timeout MUST be folded in exactly as [`is_zombie`]
/// does: a review-gated task whose run legitimately ran longer than the global
/// TTL (its own `timeout_secs` allows up to 24h) would otherwise be warned
/// "review-stalled" the moment it parks — and the once-per-task dedup burns
/// the single observability signal on that false alarm. A `ttl_secs` of 0
/// disables the check, reusing the zombie-detection kill-switch.
#[must_use]
pub fn is_stale_review(task: &CoordTask, now_epoch: u64, ttl_secs: u64) -> bool {
    if ttl_secs == 0 {
        return false;
    }
    if task.status != CoordTaskStatus::WaitingReview {
        return false;
    }
    if !is_dispatcher_managed(task) {
        return false;
    }
    let Some(started) = task.started_at else {
        return false;
    };
    let effective_ttl = ttl_secs.max(read_task_timeout(&task.metadata).unwrap_or(0));
    now_epoch.saturating_sub(started) > effective_ttl
}

/// How [`TeamDispatcher::run_task`] may finalise a finished member run, given
/// what changed under it while the member was executing.
///
/// The run works from a claim-time snapshot, so two classes of concurrent
/// change would otherwise be silently overwritten when it lands:
///
/// * an **admin action** moved the task out of `InProgress` (cancel, manual
///   retry back to Pending, skip, …) — writing our outcome would swallow the
///   operator's decision (a cancel resurrected, a requested retry replaced by
///   a stale Completed);
/// * a **successor claim** already started a newer run — possible once
///   `release_stale_locks` frees a long run's lock (`lock_ttl_secs` <
///   per-task timeout) and an admin reset makes the row claimable again.
///   Writing anything would clobber the successor's state, release *its*
///   lock (same owner string), and evict *its* running-map entry, which the
///   orphan janitor would then "reclaim" into a duplicate third run.
///
/// This is the run-id fence hermes-agent uses (`expected_run_id` CAS): the
/// mechanical guarantee that exactly one writer finalises a task row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::teams::dispatcher) enum RunFinalize {
    /// Nothing changed under us — apply the outcome normally.
    Proceed,
    /// The task left `InProgress` while we ran. Keep the newer (foreign)
    /// state: skip the terminal write, but release our own claim (lock +
    /// running-map entry) — no successor exists yet, so both are still ours.
    KeepForeignState(CoordTaskStatus),
    /// A newer run superseded ours. Touch nothing: task row, lock, and
    /// running-map entry all belong to the successor now.
    Superseded,
}

/// Pure fence decision for [`RunFinalize`]. `my_run_id` empty (run recording
/// unavailable) disables the supersession check; `current_status`/`latest_run_id`
/// of `None` (re-fetch failed / row gone) fall through to `Proceed` — P7
/// graceful degradation, identical to the pre-fence behaviour.
#[must_use]
pub(in crate::teams::dispatcher) fn finalize_disposition(
    my_run_id: &str,
    latest_run_id: Option<&str>,
    current_status: Option<CoordTaskStatus>,
) -> RunFinalize {
    if !my_run_id.is_empty() && latest_run_id.is_some_and(|latest| latest != my_run_id) {
        return RunFinalize::Superseded;
    }
    match current_status {
        Some(s) if s != CoordTaskStatus::InProgress => RunFinalize::KeepForeignState(s),
        _ => RunFinalize::Proceed,
    }
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

    /// A pause that landed while the step was running must survive the crash
    /// it could not stop. Before this branch the orphan reclaim reset the row
    /// to Pending and the dispatcher re-ran the paused step on the next tick —
    /// no error, no log, and `workflow status` still said "paused".
    #[test]
    fn an_orphan_paused_mid_run_parks_instead_of_redispatching() {
        use crate::agents::swarm::tasks::{
            merge_metadata_patch, PAUSED_FROM_IN_PROGRESS, PAUSED_FROM_KEY,
        };

        let plain = task(
            "t1",
            CoordTaskStatus::InProgress,
            Some("a"),
            true,
            Priority::Normal,
            1,
        );
        assert_eq!(orphan_reset_status(&plain), CoordTaskStatus::Pending);

        let mut paused_mid_run = plain.clone();
        paused_mid_run.metadata = merge_metadata_patch(
            &paused_mid_run.metadata,
            serde_json::json!({ PAUSED_FROM_KEY: PAUSED_FROM_IN_PROGRESS }),
        );
        assert_eq!(
            orphan_reset_status(&paused_mid_run),
            CoordTaskStatus::Paused,
            "the user's pause must not be undone by a restart"
        );

        // A review-origin stamp is a different pause (the row was already
        // parked); it never reaches this path, and must not park an orphan.
        let mut review_stamped = plain;
        review_stamped.metadata = merge_metadata_patch(
            &review_stamped.metadata,
            serde_json::json!({ PAUSED_FROM_KEY: "waiting_review" }),
        );
        assert_eq!(
            orphan_reset_status(&review_stamped),
            CoordTaskStatus::Pending
        );
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

    // ---- Run-finalize fence -------------------------------------------------

    #[test]
    fn finalize_proceeds_when_nothing_changed() {
        assert_eq!(
            finalize_disposition("r1", Some("r1"), Some(CoordTaskStatus::InProgress)),
            RunFinalize::Proceed
        );
    }

    #[test]
    fn finalize_detects_superseding_run() {
        // A newer claim recorded run r2 while we (r1) were still executing —
        // the cancel→manual-retry race on a stale-lock-released long run.
        assert_eq!(
            finalize_disposition("r1", Some("r2"), Some(CoordTaskStatus::InProgress)),
            RunFinalize::Superseded
        );
        // Supersession wins even over a foreign status: everything is the
        // successor's now, including the right to react to that status.
        assert_eq!(
            finalize_disposition("r1", Some("r2"), Some(CoordTaskStatus::Cancelled)),
            RunFinalize::Superseded
        );
    }

    #[test]
    fn finalize_keeps_foreign_state_on_admin_transition() {
        // Cancelled mid-flight (the pre-fence guard) …
        assert_eq!(
            finalize_disposition("r1", Some("r1"), Some(CoordTaskStatus::Cancelled)),
            RunFinalize::KeepForeignState(CoordTaskStatus::Cancelled)
        );
        // … and the previously-swallowed case: an operator hard-retried the
        // task back to Pending while our (cancelled) run was still landing.
        assert_eq!(
            finalize_disposition("r1", Some("r1"), Some(CoordTaskStatus::Pending)),
            RunFinalize::KeepForeignState(CoordTaskStatus::Pending)
        );
    }

    #[test]
    fn finalize_degrades_gracefully_without_signals() {
        // No run recording (empty id) → supersession check disabled.
        assert_eq!(
            finalize_disposition("", Some("r2"), Some(CoordTaskStatus::InProgress)),
            RunFinalize::Proceed
        );
        // Re-fetches failed (P7) → proceed exactly as before the fence.
        assert_eq!(finalize_disposition("r1", None, None), RunFinalize::Proceed);
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

    #[test]
    fn zombie_detection_honours_longer_per_task_timeout() {
        use crate::agents::swarm::tasks::timeout::with_task_timeout;
        // A task with a per-task timeout (1000s) above the global zombie TTL (60s)
        // must not be reaped before its own deadline — the effective threshold is
        // max(global, per-task). Started 500s ago: past the global TTL but inside
        // its own 1000s budget → not yet a zombie.
        let mut t = in_progress_task("long", 1_000_000, true);
        t.metadata = with_task_timeout(t.metadata, Some(1000));
        let running = HashSet::new();
        assert!(!is_zombie(&t, &running, 1_000_500, 60));
        // Past its own budget → finally a zombie.
        assert!(is_zombie(&t, &running, 1_001_001, 60));
    }

    #[test]
    fn zombie_detection_ignores_shorter_per_task_timeout() {
        use crate::agents::swarm::tasks::timeout::with_task_timeout;
        // A per-task timeout *below* the global TTL never shrinks the safety net:
        // the global zombie grace still governs (effective = max(global, per-task)).
        let mut t = in_progress_task("short", 1_000_000, true);
        t.metadata = with_task_timeout(t.metadata, Some(10));
        let running = HashSet::new();
        // 30s elapsed: past the 10s per-task timeout but inside the 60s global
        // grace → not a zombie (the in-process timeout already aborts the run;
        // the zombie reaper is only the orphan safety net).
        assert!(!is_zombie(&t, &running, 1_000_030, 60));
    }

    // ---- Stale review detection -------------------------------------------

    /// Build a `WaitingReview` dispatcher-managed task with `started_at` set.
    fn waiting_review_task(id: &str, started_at: u64, managed: bool) -> CoordTask {
        let mut t = task(
            id,
            CoordTaskStatus::WaitingReview,
            Some("a"),
            managed,
            Priority::Normal,
            0,
        );
        t.started_at = Some(started_at);
        t
    }

    #[test]
    fn stale_review_disabled_when_ttl_zero() {
        let t = waiting_review_task("stuck", 1000, true);
        assert!(!is_stale_review(&t, 1_000_000, 0));
    }

    #[test]
    fn stale_review_ignores_non_waiting_review_status() {
        // An InProgress task is the zombie reaper's job, not this one.
        let t = in_progress_task("running", 1000, true);
        assert!(!is_stale_review(&t, 1_000_000, 60));
    }

    #[test]
    fn stale_review_ignores_unmanaged_task() {
        // team_delegate-owned review tasks are the caller's responsibility.
        let t = waiting_review_task("delegated", 1000, false);
        assert!(!is_stale_review(&t, 1_000_000, 60));
    }

    #[test]
    fn stale_review_ignores_task_without_started_at() {
        let mut t = waiting_review_task("no_clock", 1000, true);
        t.started_at = None;
        assert!(!is_stale_review(&t, 1_000_000, 60));
    }

    #[test]
    fn stale_review_respects_grace_window() {
        let t = waiting_review_task("young", 1_000_000, true);
        // parked 30s ago, ttl 60s → not yet stale
        assert!(!is_stale_review(&t, 1_000_030, 60));
    }

    #[test]
    fn stale_review_fires_past_grace_window() {
        let t = waiting_review_task("old", 1_000_000, true);
        // parked 61s ago, ttl 60s → stale
        assert!(is_stale_review(&t, 1_000_061, 60));
    }

    #[test]
    fn stale_review_honours_longer_per_task_timeout() {
        use crate::agents::swarm::tasks::timeout::with_task_timeout;
        // Mirror of `zombie_detection_honours_longer_per_task_timeout`:
        // staleness is anchored to the RUN start, so a review-gated task whose
        // run legitimately consumed its own (longer) budget must not be warned
        // the moment it parks — effective TTL = max(global, per-task).
        let mut t = waiting_review_task("long_run", 1_000_000, true);
        t.metadata = with_task_timeout(t.metadata, Some(1000));
        // 500s since run start: past the 60s global TTL but inside the task's
        // own 1000s budget → not stale (the review wait may be ~0s).
        assert!(!is_stale_review(&t, 1_000_500, 60));
        // Past even the per-task budget → finally stale.
        assert!(is_stale_review(&t, 1_001_001, 60));
    }
}

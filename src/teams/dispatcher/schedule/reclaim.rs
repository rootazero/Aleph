//! Janitor duties: reap zombie (worker-lost) and orphaned (crash-left) tasks
//! back to a schedulable or terminal state.

use std::collections::{HashMap, HashSet};

use crate::agents::swarm::tasks::acceptance::{
    read_stale_review_warned_at, with_stale_review_warned_at,
};
use crate::agents::swarm::tasks::retry::{read_retry_budget_reset_at, recovery_abandons_since};
use crate::agents::swarm::tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
use crate::session::boundary_repair::repair_and_close_abandoned;
use crate::session::service::{SessionError, SessionId};
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

use super::select::{is_dispatcher_managed, is_stale_review, is_zombie, orphan_reset_status};
use super::TeamDispatcher;
use crate::teams::dispatcher::MAX_TASK_RECOVERIES;

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
    ///
    /// Two things this pass is NOT allowed to do silently:
    ///
    /// * **Recover forever.** A crash orphan spends no retry budget (correct —
    ///   it is not the task's fault) and the reset re-stamps `started_at`, so
    ///   `zombie_ttl_secs` never sees the age accumulate. A task that reliably
    ///   kills its worker was therefore re-dispatched without any bound.
    ///   [`MAX_TASK_RECOVERIES`](crate::teams::dispatcher::MAX_TASK_RECOVERIES)
    ///   is that bound; past it the task is failed terminally **with the
    ///   reason spelled out**, because this is the first path from which
    ///   [`fail_task`](Self::fail_task) is reachable at boot — the settle sweep
    ///   will push that text to the launching user as the run's terminal
    ///   summary, and an unattributed `Failed` there is worse than none.
    /// * **Undo a pause.** `workflow(action='pause')` cannot stop an in-flight
    ///   step, so it records the *intent* on the row
    ///   (`paused_from = "in_progress"`) and leaves the status alone. A blind
    ///   reset to `Pending` here would re-dispatch a step the user paused —
    ///   silently, on the next boot. Such a task parks `Paused` instead; the
    ///   stamp rides along and `workflow(action='resume')` clears it.
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

            // Crash-recovery budget. Read BEFORE the lock release / status
            // write so the decision is made against the same row snapshot the
            // reclaim acts on. An unreadable run log recovers (this pass is a
            // safety net, not a judge — degrading to "give up" would turn a
            // transient store hiccup into a dead workflow); the bound simply
            // does not apply for that tick.
            let recoveries = match self.coord_store.list_task_runs(&task.id).await {
                Ok(runs) => {
                    recovery_abandons_since(&runs, read_retry_budget_reset_at(&task.metadata))
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, error = %e,
                        "dispatcher: run history unreadable; recovering without counting");
                    0
                }
            };
            if recoveries >= MAX_TASK_RECOVERIES {
                tracing::warn!(
                    task_id = %task.id,
                    recoveries,
                    max_recoveries = MAX_TASK_RECOVERIES,
                    "dispatcher: task worker died repeatedly; abandoning instead of re-dispatching"
                );
                if let Some(holder) = &task.locked_by {
                    if let Err(e) = self.coord_store.release_lock(&task.id, holder).await {
                        tracing::warn!(task_id = %task.id, error = %e, "dispatcher: release_lock failed during recovery-budget fail");
                    }
                }
                // Attributed on purpose: this string becomes the step's
                // `result`, which the settle sweep renders into the terminal
                // summary pushed to the launching user.
                self.fail_task(
                    &task,
                    &format!(
                        "abandoned after {recoveries} crash recovery attempt(s) \
                         (limit {MAX_TASK_RECOVERIES}): the worker running this step \
                         died before finishing, every time"
                    ),
                )
                .await;
                continue;
            }

            if let Some(holder) = &task.locked_by {
                if let Err(e) = self.coord_store.release_lock(&task.id, holder).await {
                    tracing::warn!(task_id = %task.id, error = %e, "dispatcher: release_lock failed during orphan reclaim");
                }
            }
            // The crash boundary of this step's own session is THIS pass's to
            // repair, because this pass is what re-dispatches the step. The
            // boot resume scan hands every team session back here
            // (`resume_coordinator::has_own_scheduler`) precisely so the two
            // never write into one log; if this pass skipped it nobody would,
            // and the next attempt's replay would silently drop every tool call
            // the crash caught in flight — while the recovery section told the
            // member to "reuse work already done".
            //
            // Before the reset, not after: once the row is `Pending` the
            // dispatcher may claim it on the same tick, and a repair racing a
            // live run would append a `RunFinished` into the middle of it.
            self.repair_and_stamp_orphan(&task).await;

            // A pause that landed while this step was in flight recorded its
            // intent and nothing else (the run could not be stopped). Honour
            // it now: park the step instead of re-dispatching it. The stamp
            // stays on the row — `workflow(action='resume')` restores Pending
            // and clears it in one write, which is the ONLY thing that returns
            // this row to janitor visibility (`Paused` is invisible to
            // `reclaim_zombies` and `abandon_orphaned_runs`, both InProgress-
            // scoped).
            let restore = orphan_reset_status(&task);
            // Surface a failed reset: a silent drop would loop this orphan
            // invisibly (it stays InProgress and is retried every tick with
            // zero log), contradicting the warn-on-error discipline every
            // sibling path in this file already follows (release_lock above,
            // list_tasks above, reclaim_zombies) — P7.
            if let Err(e) = self
                .coord_store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        status: Some(restore),
                        ..Default::default()
                    },
                )
                .await
            {
                tracing::warn!(task_id = %task.id, error = %e, status = %restore,
                    "dispatcher: reclaim_orphaned status reset failed");
            }
        }
    }

    /// Repair the crashed attempt's session log and stamp what it produced
    /// onto its own run row.
    ///
    /// Best-effort in every direction (P7): a task with no owner (nothing was
    /// ever dispatched, so there is no session), a process with no event store
    /// wired, an unreadable log — each leaves the reclaim itself untouched,
    /// because resetting the row to `Pending` is the part that must happen.
    /// Every give-up is logged; none of them is silent.
    async fn repair_and_stamp_orphan(self: &Arc<Self>, task: &CoordTask) {
        let Some(owner) = task.owner.as_deref() else {
            return; // no owner ⇒ no member session key ⇒ nothing was dispatched
        };
        let Some(store) = crate::session::store::global_session_event_store() else {
            return; // persistence not wired (embeddings, most unit tests)
        };
        let session = crate::routing::session_key::SessionKey::task(
            owner,
            crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
            &task.id,
        );
        let summary = match repair_crashed_attempt(store.as_ref(), &session).await {
            Ok(Some(summary)) => summary,
            Ok(None) => return, // the attempt left nothing behind to report
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e,
                    "dispatcher: crash-boundary repair of the member session failed");
                return;
            }
        };
        if let Err(e) = self
            .coord_store
            .stamp_abandoned_run_summary(&task.id, &summary)
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e,
                "dispatcher: stamping the crashed attempt's partial output failed");
        }
    }

    /// Surface tasks silently stalled in `WaitingReview` past the TTL.
    ///
    /// The sibling janitor to [`Self::reclaim_zombies`] for the review gate. A
    /// review-gated task parks in `WaitingReview` after its run and waits for the
    /// lead to call `workflow_step_review` / `task_review`. Nothing reaps it —
    /// both reclaim passes only touch `InProgress` — so a review that never
    /// arrives stalls the task and its whole downstream DAG branch **silently**.
    /// This pass makes that visible: once per park, for each task whose wait
    /// exceeds `zombie_ttl_secs` (reused as the review-stall threshold — no new
    /// knob), it logs a warning AND stamps `stale_review_warned_at` into the
    /// task metadata. The stamp is doing double duty:
    ///
    /// * **Durable dedup** — a stamp at/after the current run's `started_at`
    ///   means this park was already warned; it survives restarts (the
    ///   in-memory set it replaced re-warned after every restart) and needs no
    ///   pruning pass (a later re-run re-stamps `started_at`, re-arming the
    ///   warning naturally).
    /// * **Leader delivery (R5)** — the metadata write goes through
    ///   `update_task`, which re-emits `TeamTaskUpdated{waiting_review}`; the
    ///   existing `TeamNotifier` turns that into a leader-inbox reminder. The
    ///   stall signal finally reaches an actor who can act on it, instead of a
    ///   server log nobody reads.
    ///
    /// It deliberately takes **no** corrective action: the approve/reject verdict
    /// is the lead LLM's call (R7); this only makes the wait visible.
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
        for task in &waiting {
            if !is_dispatcher_managed(task) {
                continue; // team_delegate-owned reviews are the caller's problem
            }
            if !is_stale_review(task, now, ttl) {
                continue;
            }
            // Once per park: a stamp at/after this run's start means the
            // current wait was already surfaced (started_at is re-stamped on
            // every re-claim, so a task that returns to review re-arms).
            let started = task.started_at.unwrap_or(0);
            if read_stale_review_warned_at(&task.metadata).is_some_and(|w| w >= started) {
                continue;
            }
            let waited = now.saturating_sub(task.started_at.unwrap_or(now));
            tracing::warn!(
                task_id = %task.id,
                waited_secs = waited,
                zombie_ttl_secs = ttl,
                "dispatcher: task stalled in waiting_review past TTL — lead review never arrived (verdict is the lead's call; not auto-resolving)"
            );
            // Stamp + re-emit (the fresh row from list_tasks is the metadata
            // base — this pass runs before any claim could race it).
            if let Err(e) = self
                .coord_store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        metadata: Some(with_stale_review_warned_at(task.metadata.clone(), now)),
                        ..Default::default()
                    },
                )
                .await
            {
                // Not stamped → next tick warns again; noisy but never silent.
                tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to stamp stale-review warning");
            }
        }
    }

    /// Close `running` run rows whose worker is gone as `Abandoned`.
    ///
    /// The task-status janitors above ([`Self::reclaim_zombies`] /
    /// [`Self::reclaim_orphaned`]) recover the TASK, but neither closes the
    /// stuck `running` row in `coord_task_runs` — and both are keyed on
    /// `InProgress`, so a cancel-then-crash orphan (task already terminal,
    /// row still `running`) is invisible to them forever. This pass keys on
    /// the runs table instead: any `running` row whose task id is not in
    /// this process's `running` set can never finish (its tokio worker died
    /// with a previous daemon incarnation). Race-free because the running-set
    /// insert precedes the spawn and removal follows `finish_task_run`, so a
    /// live row is always covered. The startup `dispatch_once` gives a free
    /// boot-time full sweep while the set is empty.
    pub(super) async fn abandon_orphaned_runs(self: &Arc<Self>) {
        let live: Vec<String> = self.running.lock().await.keys().cloned().collect();
        match self.coord_store.abandon_orphaned_runs(&live).await {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(
                    closed = n,
                    "dispatcher: closed orphaned running run rows as abandoned"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: abandon_orphaned_runs failed");
            }
        }
    }

    /// Stop the member runs of tasks that went terminal while still running.
    ///
    /// `workflow(action='cancel')` and `team_task_control(action='cancel')`
    /// write `Cancelled` on the task row — and that is all they could do: the
    /// run is a live agent loop owned by the execution engine, which neither
    /// tool holds. So a cancelled step kept burning tokens (and tool calls) for
    /// up to `task_timeout_secs`, on some deployments 24 hours, while the panel
    /// showed it cancelled.
    ///
    /// Deliberately a **janitor here**, not an `ExecutionAdapter` injected into
    /// the two cancel tools. There are already two cancel faces and the panel
    /// RPCs make more; wiring each one to the engine means the third and fourth
    /// silently miss it. The dispatcher is the one place that knows which runs
    /// are actually in flight (`self.running`), so it is the one place the
    /// answer can be complete. Mechanical throughout (R10): "the task is
    /// terminal but its run is live" is a structural fact, not a judgement.
    ///
    /// Fail-soft on a store error: a run is only stopped on a **positive**
    /// reading of a terminal status, never on an unreadable one. A benign race
    /// exists at the tail of a healthy run (status written Completed, entry not
    /// yet evicted from `self.running`) — by then the engine run has already
    /// deregistered, so `cancel_session` finds nothing and returns `Ok(None)`.
    pub(super) async fn cancel_runs_of_terminal_tasks(self: &Arc<Self>) {
        let running: Vec<(String, String)> = self
            .running
            .lock()
            .await
            .iter()
            .map(|(id, owner)| (id.clone(), owner.clone()))
            .collect();

        for (task_id, owner) in running {
            // `is_terminal`, not `is_settled`: an `Unsatisfiable` row is a
            // dependency corpse, and a task that is actually running cannot be
            // one — widening the predicate here would only add a way to be
            // wrong.
            let terminal = match self.coord_store.get_task(&task_id).await {
                Ok(Some(t)) => t.status.is_terminal(),
                // The row is gone entirely — nothing this run can still land on.
                Ok(None) => true,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: cancel sweep get_task failed; leaving the run alone");
                    continue;
                }
            };
            if !terminal {
                continue;
            }
            let session_key = crate::routing::session_key::SessionKey::task(
                &owner,
                crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
                &task_id,
            );
            match self
                .context
                .execution_adapter()
                .cancel_session(&session_key)
                .await
            {
                Ok(Some(run_id)) => {
                    tracing::info!(task_id = %task_id, run_id = %run_id, "dispatcher: task went terminal mid-run; cancelled its member run");
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "dispatcher: could not cancel the member run of a terminal task");
                }
            }
        }
    }

    /// Re-deliver clarify questions stranded by a crash between park and send.
    ///
    /// `handle_clarify_task` parks the task `Paused` (with a
    /// `clarify_delivery_pending` stamp) BEFORE delivering the question — if
    /// the daemon dies in that window, the row stays Paused with a question
    /// nobody ever received, dependents stay Blocked, and no other janitor
    /// looks at Paused. This pass resets such tasks (past a short grace, so a
    /// live in-flight delivery is never raced) back to `Pending`; the normal
    /// selection loop then re-runs the delivery. A clarify task whose
    /// delivery WAS confirmed carries `clarify_delivered_at` instead and is
    /// left parked awaiting its answer; one an operator manually paused
    /// carries neither stamp and is never touched (their pause, their
    /// resume). Purely mechanical (R10 — a crash-recovery reset, no
    /// reasoning).
    pub(super) async fn redeliver_stalled_clarify(self: &Arc<Self>) {
        /// A pending stamp younger than this is assumed to be a live delivery
        /// still in flight on this process; older means the delivering tick
        /// died. Generous — a channel send is sub-second.
        const REDELIVERY_GRACE_SECS: u64 = 60;

        let paused = match self
            .coord_store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::Paused),
                ..Default::default()
            })
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher: redeliver_stalled_clarify list_tasks failed");
                return;
            }
        };

        let now = Self::now_epoch();
        for task in paused {
            if !is_dispatcher_managed(&task) || !crate::workflow::clarify::is_clarify_task(&task) {
                continue;
            }
            let Some(pending_at) =
                crate::workflow::clarify::clarify_delivery_pending_at(&task.metadata)
            else {
                continue; // delivered (awaiting answer) or manually paused
            };
            if now.saturating_sub(pending_at) < REDELIVERY_GRACE_SECS {
                continue; // possibly a live delivery on this very tick
            }
            tracing::warn!(
                task_id = %task.id,
                parked_secs = now.saturating_sub(pending_at),
                "dispatcher: clarify question parked but never delivered (crash window) — resetting for re-delivery"
            );
            let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
                &task.metadata,
                serde_json::json!({
                    crate::workflow::CLARIFY_DELIVERY_PENDING_KEY: serde_json::Value::Null
                }),
            );
            if let Err(e) = self
                .coord_store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        status: Some(CoordTaskStatus::Pending),
                        metadata: Some(cleared),
                        ..Default::default()
                    },
                )
                .await
            {
                tracing::warn!(task_id = %task.id, error = %e, "dispatcher: clarify re-delivery reset failed");
            }
        }
    }
}

/// Answer the dangling calls of a crash-interrupted member run, close the run
/// marker it left open, and render what it managed to produce.
///
/// A free function over the store rather than a method so the rule is testable
/// without a dispatcher, a registry and the process-wide event-store slot; the
/// caller resolves both.
///
/// The sentence is the *dispatcher's* vocabulary on purpose — `session::
/// boundary_repair` knows nothing about "attempts" or "steps", and
/// `build_recovery_section` prints this text under "partial output
/// (incomplete)" to the NEXT attempt of the same step.
///
/// # Errors
///
/// Propagates the store's errors and the reducer's refusal. `Ok(None)` means
/// the attempt genuinely left nothing behind, which is a different answer from
/// "its log could not be read" — collapsing the two would stamp an empty
/// summary onto a row whose real state is unknown.
pub(super) async fn repair_crashed_attempt(
    store: &dyn SessionEventStore,
    session: &SessionId,
) -> Result<Option<String>, SessionError> {
    let report = repair_and_close_abandoned(store, session).await?;
    let progress = &report.progress;
    if progress.tool_calls_dispatched == 0
        && progress.assistant_messages == 0
        && report.appended == 0
    {
        return Ok(None);
    }

    let mut out = format!(
        "interrupted by a daemon restart after {} assistant message(s), \
         {}/{} tool call(s) answered",
        progress.assistant_messages, progress.tool_calls_answered, progress.tool_calls_dispatched
    );
    if report.appended > 0 {
        out.push_str(&format!(
            "; {} call(s) were still in flight when it died and their outcome is unknown",
            report.appended
        ));
    }
    if let Some(text) = report.last_assistant.as_deref() {
        out.push_str("\nLast recorded output:\n");
        out.push_str(text);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod crashed_attempt_tests {
    use super::repair_crashed_attempt;
    use crate::session::events::{MessageContent, SessionEvent, TurnId};
    use crate::session::service::SessionId;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use std::sync::Arc;

    fn store_and_session() -> (Arc<dyn SessionEventStore>, SessionId) {
        let conn = rusqlite::Connection::open_in_memory().expect("sqlite");
        migrate_add_session_events(&conn).expect("migrate");
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let sid = crate::routing::session_key::SessionKey::task(
            "worker",
            crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
            format!("task-{}", uuid::Uuid::new_v4()),
        );
        (store, sid)
    }

    async fn seed(store: &Arc<dyn SessionEventStore>, sid: &SessionId, evs: &[SessionEvent]) {
        for (i, ev) in evs.iter().enumerate() {
            store
                .append(sid, (i + 1) as u64, ev, 10 * (i as i64 + 1))
                .await
                .expect("append");
        }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: 20,
        }
    }

    /// The re-dispatching scheduler repairs the crash boundary of the attempt
    /// it is about to replace, and hands back a sentence with enough in it to
    /// be worth printing under "partial output (incomplete)".
    ///
    /// RED without the `repair_and_close_abandoned` call: the log keeps a
    /// `tool_use` with no `tool_result` (the next attempt's replay drops it
    /// silently) and an open `RunStarted` (every later boot re-classifies the
    /// session `Interrupted`).
    #[tokio::test]
    async fn a_crashed_member_run_is_repaired_closed_and_summarised() {
        let (store, sid) = store_and_session();
        seed(
            &store,
            &sid,
            &[
                SessionEvent::RunStarted {
                    run_id: "member-run".into(),
                    at: 10,
                    project_root: None,
                    envelope: None,
                },
                assistant("read the spec, starting the edit"),
                SessionEvent::ToolCallRequested {
                    turn_id: TurnId::new_v4(),
                    call_id: "c1".into(),
                    name: "file_write".into(),
                    input: serde_json::json!({}),
                    at: 30,
                },
            ],
        )
        .await;

        let summary = repair_crashed_attempt(store.as_ref(), &sid)
            .await
            .expect("legal log")
            .expect("the attempt left something behind");
        assert!(
            summary.contains("0/1 tool call(s) answered"),
            "the counters name what actually landed: {summary}"
        );
        assert!(
            summary.contains("outcome is unknown"),
            "the in-flight call is named as unknown, not as landed: {summary}"
        );
        assert!(
            summary.contains("read the spec, starting the edit"),
            "the member's own last words are the partial output: {summary}"
        );

        let events = store.load_all_events(&sid).await.expect("load");
        let repair_at = events
            .iter()
            .position(
                |r| matches!(&r.event, SessionEvent::ToolError { call_id, .. } if call_id == "c1"),
            )
            .expect("the dangling call was answered");
        // The verdict, not merely the marker: a `RunFinished` carrying any
        // other outcome would tell the next reader this attempt reached a
        // terminal state of its own, when in fact its process disappeared.
        let closed_at = events
            .iter()
            .position(|r| {
                matches!(
                    &r.event,
                    SessionEvent::RunFinished { run_id, outcome, .. }
                        if run_id == "member-run"
                            && *outcome == crate::session::events::RunOutcome::Abandoned
                )
            })
            .expect("the run marker the crash left open was closed as Abandoned");
        // Order matters as much as presence: closing the run first would put
        // the repair for a call the run dispatched *after* its own end marker,
        // and the next reduction would charge it to whatever run comes next.
        assert!(
            repair_at < closed_at,
            "the orphan tool call is answered before the run marker closes \
             (repair at {repair_at}, close at {closed_at})"
        );
    }

    /// A step whose worker died before it did anything has nothing to say, and
    /// an empty summary must not be stamped onto its row: "this attempt left
    /// nothing behind" and "I could not read its log" are different answers.
    #[tokio::test]
    async fn an_attempt_that_produced_nothing_reports_nothing() {
        let (store, sid) = store_and_session();
        seed(
            &store,
            &sid,
            &[SessionEvent::RunStarted {
                run_id: "member-run".into(),
                at: 10,
                project_root: None,
                envelope: None,
            }],
        )
        .await;

        assert_eq!(
            repair_crashed_attempt(store.as_ref(), &sid)
                .await
                .expect("legal log"),
            None
        );
    }
}

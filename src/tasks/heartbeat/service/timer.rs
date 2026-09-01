//! Timer loop and tick execution for heartbeat tasks.
//!
//! The timer loop is the heartbeat of the heartbeat service (meta!). It wakes
//! up periodically or on `WakeQueue` notification, finds due tasks, runs L1 probes,
//! optionally triggers L2 agent analysis, handles dedup and delivery, and writes
//! back results.

use crate::sync_primitives::Arc;
use tokio::sync::Semaphore;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info};

use crate::tasks::cron::config::RunStatus;
use crate::tasks::heartbeat::config::HeartbeatTask;
use crate::tasks::heartbeat::dedup::{DedupEngine, DedupVerdict};
use crate::tasks::heartbeat::executor::{
    build_heartbeat_prompt, HeartbeatExecutionAdapter, HeartbeatL2Status,
};
use crate::tasks::heartbeat::history::HeartbeatRunRecord;
use crate::tasks::heartbeat::probe::{execute_probe, ProbeExecutor};
use crate::tasks::heartbeat::service::state::HeartbeatServiceState;
use crate::tasks::heartbeat::wake::{WakeQueue, WakeRequest};
use crate::tasks::shared::alert::failure_alert_payload;
use crate::tasks::shared::delivery::DeliveryStatus;
use crate::tasks::shared::delivery::{DeliveryEngine, DeliveryPayload};

// ── TickContext ───────────────────────────────────────────────────────

/// Emitter for `HeartbeatTaskChanged` (state) frames after a timer-tick
/// writeback.
///
/// The webchat panel dropped polling and relies on `heartbeat.task.changed`
/// push; without this, scheduled runs completing silently leave the panel
/// stale. Wired at startup from the `HeartbeatService`'s event bus. Takes the
/// task id.
pub type ChangeEmitterFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Shared context for each tick execution, holding all external dependencies.
pub struct TickContext {
    pub probe_executor: Arc<dyn ProbeExecutor>,
    pub adapter: Arc<dyn HeartbeatExecutionAdapter>,
    pub delivery: Arc<DeliveryEngine>,
    pub dedup: Arc<DedupEngine>,
    /// Per-task execution timeout (seconds) — bounds both the L1 probe and
    /// the L2 agent turn so a hung tool cannot stall a worker forever.
    pub job_timeout_secs: u64,
    /// Optional change emitter published after each successful writeback so
    /// the panel refreshes a task's runtime state (H-push).
    pub change_emitter: Option<ChangeEmitterFn>,
}

// ── HeartbeatTickResult ──────────────────────────────────────────────

/// Result of executing a single heartbeat task tick.
#[derive(Debug)]
pub struct HeartbeatTickResult {
    /// "Triggered" / "Skipped" / "Error"
    pub l1_status: String,
    /// "Silent" / "Delivered" / "Deduped" / "Error" / None
    pub l2_status: Option<String>,
    pub l1_duration_ms: i64,
    pub l2_duration_ms: Option<i64>,
    pub error: Option<String>,
    pub delivery_status: Option<String>,
    /// Stringified raw probe value for Changed tracking
    pub new_probe_result: Option<String>,
}

// ── Timer Loop ───────────────────────────────────────────────────────

/// Run the heartbeat timer loop until shutdown is requested.
///
/// Each iteration:
/// 1. Sleep for `tick_interval_secs` OR wake on `WakeQueue` notification
/// 2. Collect due tasks (marking them running) — disjoint sets per tick
/// 3. Spawn each task detached, bounded by a shared semaphore
///
/// Task execution and writeback happen in detached tasks, so a slow L2 turn
/// never blocks the loop or delays unrelated tasks.
pub async fn run_heartbeat_loop(
    state: Arc<HeartbeatServiceState>,
    wake_queue: Arc<WakeQueue>,
    ctx: Arc<TickContext>,
) {
    // Guard against absurd config values (H6): a zero interval would spin
    // and a zero-permit semaphore would starve every task.
    let interval = Duration::from_secs(state.config.tick_interval_secs.max(1));
    let semaphore = Arc::new(Semaphore::new(state.config.max_concurrent.max(1)));
    info!(
        tick_interval_secs = state.config.tick_interval_secs,
        max_concurrent = state.config.max_concurrent,
        "heartbeat timer loop started"
    );

    // Startup recovery: clear running markers left behind by a crash so the
    // affected tasks are not skipped forever (H10).
    clear_stale_running_markers(&state).await;

    loop {
        tokio::select! {
            _ = sleep(interval) => {},
            _ = wake_queue.notified() => {},
        }

        if state.is_shutdown() {
            info!("heartbeat timer loop: shutdown requested, exiting");
            break;
        }

        let wake_requests = wake_queue.drain();
        let due_tasks = collect_due_tasks(&state, &wake_requests).await;
        if due_tasks.is_empty() {
            continue;
        }
        debug!(count = due_tasks.len(), "heartbeat tick: found due tasks");

        // Spawn each task detached so a slow L2 turn never blocks the timer
        // loop or other tasks (H4). Concurrency is bounded by a semaphore
        // shared across ticks; the permit is acquired *inside* the spawned
        // future so the loop returns to `select!` immediately. The per-task
        // `running_at_ms` marker set by `collect_due_tasks` keeps the next
        // tick from collecting a task that is still in flight.
        for (task, wake_reason) in due_tasks {
            let ctx = ctx.clone();
            let state = state.clone();
            let semaphore = semaphore.clone();
            tokio::spawn(async move {
                let task_id = task.id.clone();
                let _permit = match semaphore.acquire_owned().await {
                    Ok(p) => p,
                    Err(e) => {
                        // The task was already marked running by
                        // `collect_due_tasks`; if we bail without running it,
                        // clear the marker so the next tick can retry instead
                        // of leaving it stuck until daemon restart.
                        error!(error = %e, task_id = %task_id, "heartbeat: failed to acquire semaphore permit; clearing running marker");
                        clear_running_marker(&state, &task_id).await;
                        return;
                    }
                };
                let result = execute_heartbeat_tick(&task, wake_reason.as_deref(), &ctx).await;
                writeback_one(&state, &task_id, result, wake_reason.is_some(), &ctx).await;
                // Push a `StateChanged` frame so the panel (which dropped
                // polling) refreshes the task's runtime state after each run.
                if let Some(emitter) = &ctx.change_emitter {
                    emitter(&task_id);
                }
            });
        }
    }
}

/// Clear running markers older than the stale threshold.
///
/// A crash mid-execution leaves `running_at_ms` set; without this the task
/// would be skipped by `collect_due_tasks` forever. Mirrors cron's startup
/// catchup. Threshold: `max(job_timeout * 2, 2h)`.
async fn clear_stale_running_markers(state: &HeartbeatServiceState) {
    let stale_threshold_ms = (state
        .config
        .job_timeout_secs
        .saturating_mul(2)
        .saturating_mul(1000)
        .min(i64::MAX as u64) as i64)
        .max(7_200_000);
    let now_ms = state.clock.now_ms();

    let mut store = state.store.lock().await;
    let mut cleared = 0_usize;
    for task in store.tasks_mut().iter_mut() {
        if let Some(running_at) = task.state.running_at_ms {
            if now_ms.saturating_sub(running_at) > stale_threshold_ms {
                task.state.running_at_ms = None;
                cleared += 1;
            }
        }
    }
    if let Err(e) = store.persist() {
        error!(error = %e, "heartbeat: failed to persist stale-marker cleanup");
    }
    if cleared > 0 {
        info!(
            cleared,
            "heartbeat: cleared stale running markers on startup"
        );
    }
}

/// Clear the running marker for a single task that was marked due but never
/// actually executed (e.g. the spawned future failed to acquire a permit).
/// Without this the task is skipped by `collect_due_tasks` until the next
/// startup stale-marker sweep.
async fn clear_running_marker(state: &HeartbeatServiceState, task_id: &str) {
    let mut store = state.store.lock().await;
    if let Some(task) = store.get_task_mut(task_id) {
        if task.state.running_at_ms.is_none() {
            return;
        }
        task.state.running_at_ms = None;
        if let Err(e) = store.persist() {
            error!(error = %e, task_id, "heartbeat: failed to persist running-marker clear");
        }
    }
}

// ── Collect Due Tasks ────────────────────────────────────────────────

/// Find all tasks that are due by timer or have been woken, and mark each
/// one as running before returning it.
async fn collect_due_tasks(
    state: &HeartbeatServiceState,
    wake_requests: &[WakeRequest],
) -> Vec<(HeartbeatTask, Option<String>)> {
    let mut store = state.store.lock().await;
    let now_ms = state.clock.now_ms();

    // First pass (immutable): identify which tasks are due AND inside their
    // configured active-hours window (if any). A task that is due but outside
    // its window is not skipped silently — its `next_due_ms` is pushed forward
    // to the next window start in the second pass below so the timer doesn't
    // re-evaluate every tick.
    let mut due_ids: Vec<(String, Option<String>)> = Vec::new();
    let mut gated_ids: Vec<(String, i64)> = Vec::new(); // (task_id, new_next_due_ms)
    for task in store.tasks() {
        if !task.enabled {
            continue;
        }
        if task.state.running_at_ms.is_some() {
            continue;
        }
        let is_due = task.state.next_due_ms.is_some_and(|t| t <= now_ms);
        let wake_reason = wake_requests
            .iter()
            .find(|w| w.task_id == task.id)
            .map(|w| {
                w.reason
                    .clone()
                    .unwrap_or_else(|| "manual wake".to_string())
            });
        if !is_due && wake_reason.is_none() {
            continue;
        }
        // Manual wake bypasses the gate — the user explicitly asked for it.
        if wake_reason.is_some() {
            due_ids.push((task.id.clone(), wake_reason));
            continue;
        }
        match &task.active_hours {
            None => due_ids.push((task.id.clone(), None)),
            Some(sched) => {
                if sched.is_open_at(now_ms) {
                    due_ids.push((task.id.clone(), None));
                } else if let Some(next_open) = sched.next_open_after(now_ms) {
                    gated_ids.push((task.id.clone(), next_open));
                } else {
                    // Permanently closed — push very far out so we stop polling
                    // it but the task survives for the operator to fix.
                    gated_ids.push((task.id.clone(), now_ms + 86_400_000));
                }
            }
        }
    }

    // Second pass (mutable): mark each due task as running *before* handing
    // it to the executor; advance gated tasks' next_due_ms past their window.
    let mut due = Vec::with_capacity(due_ids.len());
    for (id, wake_reason) in due_ids {
        if let Some(task) = store.get_task_mut(&id) {
            task.state.running_at_ms = Some(now_ms);
            due.push((task.clone(), wake_reason));
        }
    }
    let had_gated = !gated_ids.is_empty();
    for (id, new_next_due) in gated_ids {
        if let Some(task) = store.get_task_mut(&id) {
            task.state.next_due_ms = Some(new_next_due);
        }
    }
    // Persist when *either* running markers or gated reschedules changed. A
    // gated-only tick (every due task is outside its active-hours window)
    // still advances `next_due_ms` in memory; without persisting it, a restart
    // re-gates the task from scratch instead of resuming past the window.
    if !due.is_empty() || had_gated {
        if let Err(e) = store.persist() {
            error!(error = %e, "heartbeat: failed to persist running markers");
        }
    }

    due
}

// ── Execute Single Tick ──────────────────────────────────────────────

/// Execute a single heartbeat task: L1 probe, optional L2 agent, dedup, delivery.
async fn execute_heartbeat_tick(
    task: &HeartbeatTask,
    wake_reason: Option<&str>,
    ctx: &TickContext,
) -> HeartbeatTickResult {
    // L1 probe (bounded by job_timeout_secs so a hung tool can't stall).
    let probe_result = execute_probe(
        &task.probe,
        ctx.probe_executor.as_ref(),
        task.state.last_probe_result.as_deref(),
        ctx.job_timeout_secs,
    )
    .await;

    match probe_result {
        Err(e) => HeartbeatTickResult {
            l1_status: "Error".into(),
            l2_status: None,
            l1_duration_ms: 0,
            l2_duration_ms: None,
            error: Some(e),
            delivery_status: None,
            new_probe_result: None,
        },
        Ok(ref r) if !r.triggered && wake_reason.is_none() => HeartbeatTickResult {
            l1_status: "Skipped".into(),
            l2_status: None,
            l1_duration_ms: r.duration_ms,
            l2_duration_ms: None,
            error: None,
            delivery_status: None,
            new_probe_result: Some(r.raw_value.to_string()),
        },
        Ok(r) => {
            // L2 execution
            let prompt = build_heartbeat_prompt(task, &r, wake_reason);
            let l2_result = ctx
                .adapter
                .execute_heartbeat(&task.agent_id, &prompt, ctx.job_timeout_secs)
                .await;

            match l2_result {
                Err(e) => HeartbeatTickResult {
                    l1_status: "Triggered".into(),
                    l2_status: Some("Error".into()),
                    l1_duration_ms: r.duration_ms,
                    l2_duration_ms: None,
                    error: Some(e),
                    delivery_status: None,
                    new_probe_result: Some(r.raw_value.to_string()),
                },
                Ok(l2) => {
                    let (l2_status, delivery_status) = match l2.status {
                        HeartbeatL2Status::Silent => ("Silent".into(), None),
                        HeartbeatL2Status::NeedsDelivery(ref output) => {
                            match ctx.dedup.check(&task.id, output).await {
                                DedupVerdict::Duplicate => ("Deduped".into(), None),
                                verdict => {
                                    let (delivered, ds) = if let Some(ref config) =
                                        task.delivery_config
                                    {
                                        let payload = DeliveryPayload {
                                            source_type: "heartbeat".into(),
                                            task_name: task.name.clone(),
                                            agent_id: task.agent_id.clone(),
                                            output: output.clone(),
                                            channel_id: None,
                                            metadata: serde_json::json!({"task_id": task.id}),
                                        };
                                        let outcomes = ctx.delivery.deliver(&payload, config).await;
                                        let ok = outcomes.iter().any(|o| o.success);
                                        let status = if ok {
                                            DeliveryStatus::Delivered
                                        } else {
                                            DeliveryStatus::NotDelivered
                                        };
                                        (ok, Some(status.label().to_string()))
                                    } else {
                                        (
                                            false,
                                            Some(DeliveryStatus::NotRequested.label().to_string()),
                                        )
                                    };
                                    // H8: only record the dedup entry once delivery
                                    // actually succeeded — a failed delivery must
                                    // stay retryable on the next run. The embedding
                                    // computed by `check` is reused (H9).
                                    if delivered {
                                        if let DedupVerdict::Fresh(ref embedding) = verdict {
                                            ctx.dedup
                                                .record_fresh(&task.id, output, embedding)
                                                .await;
                                        }
                                    }
                                    // Derive the label from what actually
                                    // happened, not from which branch we are
                                    // standing in. This arm used to hard-code
                                    // "Delivered", so a failed send — or a
                                    // task with no delivery target configured
                                    // at all — wrote a history row claiming
                                    // delivery next to a `delivery_status` of
                                    // NotDelivered/NotRequested, and the L2
                                    // outcome column was a constant.
                                    let label = if delivered {
                                        "Delivered"
                                    } else {
                                        "NotDelivered"
                                    };
                                    (label.into(), ds)
                                }
                            }
                        }
                        HeartbeatL2Status::Error(ref e) => ("Error".into(), Some(e.clone())),
                    };

                    HeartbeatTickResult {
                        l1_status: "Triggered".into(),
                        l2_status: Some(l2_status),
                        l1_duration_ms: r.duration_ms,
                        l2_duration_ms: Some(l2.duration_ms),
                        error: None,
                        delivery_status,
                        new_probe_result: Some(r.raw_value.to_string()),
                    }
                }
            }
        }
    }
}

// ── Writeback ────────────────────────────────────────────────────────

/// Map an L2 outcome label to a run status.
///
/// `NotDelivered` splits on *why*: a task with no delivery target configured
/// asked for nothing and got nothing (`NotRequested` → Ok), whereas a
/// configured target that refused the payload is a real failure of the
/// monitoring path and must feed the backoff ladder and the alert gate.
///
/// The split itself is `DeliveryStatus::delivery_failed` — shared with cron,
/// which used to answer the opposite way (a refused delivery counted as a
/// successful run). Two schedulers, one question, one answer.
fn l2_run_status(l2_status: &str, delivery_status: Option<&str>) -> Option<RunStatus> {
    match l2_status {
        "Silent" | "Delivered" => Some(RunStatus::Ok),
        "Deduped" => Some(RunStatus::Skipped),
        "Error" => Some(RunStatus::Error),
        "NotDelivered" => {
            // An unparseable label is treated as a genuine non-delivery: it can
            // only come from a producer that stopped using `label()`, and
            // guessing "fine" there is how this class of bug hides.
            let failed = delivery_status
                .and_then(DeliveryStatus::from_label)
                .is_none_or(DeliveryStatus::delivery_failed);
            Some(if failed {
                RunStatus::Error
            } else {
                RunStatus::Ok
            })
        }
        unknown => {
            tracing::warn!(l2_status = %unknown, "unrecognized heartbeat L2 status");
            None
        }
    }
}

/// Resolve the alert config for a failing task.
///
/// Explicit config wins. Otherwise, when `notify_on_failure_default` is on and
/// the task has somewhere to deliver to, synthesize the same shape cron uses
/// (after=1, cooldown=1h) pointed at the task's own primary target.
fn resolve_alert_config(
    task: &HeartbeatTask,
    notify_default: bool,
) -> Option<crate::tasks::shared::alert::FailureAlertConfig> {
    if let Some(cfg) = task.failure_alert.clone() {
        return Some(cfg);
    }
    if !notify_default {
        return None;
    }
    let target = task.delivery_config.as_ref()?.targets.first()?.clone();
    Some(crate::tasks::shared::alert::FailureAlertConfig {
        after: 1,
        cooldown_ms: 3_600_000,
        target,
    })
}

/// Prepend an alert whose own delivery failed onto the next one that is due.
///
/// This is the "report it later" half of the alert-target coupling fallback:
/// the alert channel and the failing channel are frequently the same object,
/// so a send failure must not simply discard the news.
fn merge_parked_alert(parked: Option<String>, fresh: String) -> String {
    match parked {
        Some(parked) => format!("{parked}\n(previously undelivered)\n{fresh}"),
        None => fresh,
    }
}

/// An alert decided inside the store lock, delivered after it is released.
struct PendingHeartbeatAlert {
    task_name: String,
    message: String,
    target: crate::tasks::shared::delivery::DeliveryTargetConfig,
}

/// Write a single task's tick result back to the store.
///
/// Called from each task's own detached future, so writebacks for different
/// tasks interleave freely — the store mutex serializes them.
async fn writeback_one(
    state: &HeartbeatServiceState,
    task_id: &str,
    tick_result: HeartbeatTickResult,
    was_wake: bool,
    ctx: &TickContext,
) {
    let mut store = state.store.lock().await;
    let now_ms = state.clock.now_ms();
    let mut pending_alert: Option<PendingHeartbeatAlert> = None;

    if let Some(task) = store.get_task_mut(task_id) {
        task.state.running_at_ms = None;
        task.state.last_probe_at_ms = Some(now_ms);

        if let Some(ref pr) = tick_result.new_probe_result {
            task.state.last_probe_result = Some(pr.clone());
        }

        // Update L2 status. Keep *this tick's* verdict in a local: reading it
        // back off `task.state` would fold in a stale status from an earlier
        // tick whenever this tick produced no L2 phase at all.
        let this_l2_status = tick_result
            .l2_status
            .as_deref()
            .and_then(|s| l2_run_status(s, tick_result.delivery_status.as_deref()));
        if tick_result.l2_status.is_some() {
            task.state.last_l2_at_ms = Some(now_ms);
            task.state.last_l2_status = this_l2_status;
        }

        let had_error = tick_result.error.is_some() || this_l2_status == Some(RunStatus::Error);
        if had_error {
            task.state.consecutive_errors = task.state.consecutive_errors.saturating_add(1);
            task.state.last_error = tick_result.error.clone().or_else(|| {
                tick_result
                    .delivery_status
                    .clone()
                    .map(|d| format!("L2 result was not delivered ({d})"))
            });

            // Failure alerting. Without this the whole monitoring subsystem
            // could fail indefinitely with `consecutive_errors` doing nothing
            // but lengthening the backoff — nobody is told. Same gate cron
            // uses (`tasks::shared::alert`), fed from heartbeat state.
            if let Some(alert_cfg) =
                resolve_alert_config(task, state.config.notify_on_failure_default)
            {
                let subject = format!("Heartbeat '{}' ({})", task.name, task.id);
                let fresh = crate::tasks::shared::alert::should_send_alert(
                    &subject,
                    crate::tasks::shared::alert::FailureStreak {
                        consecutive_errors: task.state.consecutive_errors,
                        last_alert_at_ms: task.state.last_failure_alert_at_ms,
                        last_error: task.state.last_error.as_deref(),
                    },
                    &alert_cfg,
                    now_ms,
                );
                if let Some(message) = fresh {
                    // Carry any earlier alert whose own delivery failed, so
                    // the first send that gets through reports the backlog too.
                    let message = merge_parked_alert(task.state.undelivered_alert.take(), message);
                    task.state.last_failure_alert_at_ms = Some(now_ms);
                    pending_alert = Some(PendingHeartbeatAlert {
                        task_name: task.name.clone(),
                        message,
                        target: alert_cfg.target,
                    });
                }
            }
        } else {
            task.state.consecutive_errors = 0;
            task.state.last_error = None;
        }

        // Recompute next due via the shared schedule helper (wall-clock +
        // interval + category-aware error backoff). Reuses `compute_next_due`
        // rather than re-deriving the formula here, so a heartbeat that just
        // recorded a 429 in `last_error` (line above) inherits the same
        // 5-minute windowed floor as the cron path.
        task.state.next_due_ms = super::ops::compute_next_due(task, now_ms);

        // Write history record.
        let run_record = HeartbeatRunRecord {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
            // `trigger_source` was always "Interval" even for `heartbeat.wake`
            // runs: the wake reason reached `execute_heartbeat_tick` (it feeds
            // the L2 prompt) but was dropped before the history write.
            trigger_source: if was_wake { "Wake" } else { "Interval" }.to_string(),
            l1_status: tick_result.l1_status.clone(),
            l2_status: tick_result.l2_status.clone(),
            started_at: now_ms
                .saturating_sub(tick_result.l1_duration_ms)
                .saturating_sub(tick_result.l2_duration_ms.unwrap_or(0)),
            ended_at: Some(now_ms),
            l1_duration_ms: Some(tick_result.l1_duration_ms),
            l2_duration_ms: tick_result.l2_duration_ms,
            error: tick_result.error.clone(),
            delivery_status: tick_result.delivery_status.clone(),
            created_at: now_ms,
        };
        if let Err(e) = store.insert_run(&run_record) {
            error!(error = %e, "failed to insert heartbeat run record");
        }
        store.mark_dirty();
    }

    if let Err(e) = store.persist() {
        error!(error = %e, "heartbeat writeback persist error");
    }
    drop(store);

    // Delivery happens outside the store lock: the alert target is often a
    // network channel and holding the mutex across it would serialize every
    // other task's writeback behind a timing-out webhook.
    if let Some(alert) = pending_alert {
        deliver_alert(state, task_id, alert, ctx).await;
    }
}

/// Deliver a failure alert, parking it for a later retry if the send fails.
///
/// The fallback matters here in a way it does not for cron: cron alerts to the
/// *source* channel of the job, whereas a heartbeat's default alert target is
/// the very delivery channel whose failure triggered the alert. So a broken
/// channel would otherwise swallow the news of its own breakage. Two levels of
/// fallback: the failure is always logged at `error!` (visible to anyone
/// reading the daemon log, which does not depend on the channel), and the
/// message is parked in `undelivered_alert` to ride along with the next alert
/// that does get through.
async fn deliver_alert(
    state: &HeartbeatServiceState,
    task_id: &str,
    alert: PendingHeartbeatAlert,
    ctx: &TickContext,
) {
    let payload = failure_alert_payload(
        "heartbeat",
        &alert.task_name,
        task_id,
        &alert.message,
    );
    let config = crate::tasks::shared::delivery::DeliveryConfig {
        mode: crate::tasks::shared::delivery::DeliveryMode::Primary,
        targets: vec![alert.target],
        fallback_target: None,
    };
    let outcomes = ctx.delivery.deliver(&payload, &config).await;
    if outcomes.iter().any(|o| o.success) {
        return;
    }

    error!(
        task_id,
        ?outcomes,
        "heartbeat failure alert could not be delivered; parking it for the next successful alert"
    );
    let mut store = state.store.lock().await;
    if let Some(task) = store.get_task_mut(task_id) {
        task.state.undelivered_alert = Some(alert.message);
        // The cooldown stamp set by the caller is deliberately *kept*. It is
        // what rate-limits alert attempts, and with the message parked above
        // nothing is lost by waiting: the next eligible alert carries this one
        // along. Clearing it here would instead have a permanently-down target
        // re-attempt delivery on every single tick.
        store.mark_dirty();
    }
    if let Err(e) = store.persist() {
        error!(error = %e, "heartbeat: failed to persist undelivered alert");
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::*;
    use crate::tasks::heartbeat::executor::{HeartbeatL2Result, HeartbeatL2Status};
    use crate::tasks::heartbeat::probe::ProbeExecutor;
    use serde_json::{json, Value};

    // Mock probe executor
    struct MockProbe {
        result: Value,
    }

    #[async_trait::async_trait]
    impl ProbeExecutor for MockProbe {
        async fn execute(
            &self,
            _tool_name: &str,
            _params: Option<&Value>,
        ) -> Result<Value, String> {
            Ok(self.result.clone())
        }
    }

    // Mock L2 adapter
    struct MockAdapter {
        status: &'static str, // "silent" or "delivery" or "error"
    }

    #[async_trait::async_trait]
    impl HeartbeatExecutionAdapter for MockAdapter {
        async fn execute_heartbeat(
            &self,
            _agent_id: &str,
            _prompt: &str,
            _timeout_secs: u64,
        ) -> Result<HeartbeatL2Result, String> {
            let status = match self.status {
                "silent" => HeartbeatL2Status::Silent,
                "delivery" => HeartbeatL2Status::NeedsDelivery("Test output".into()),
                "error" => HeartbeatL2Status::Error("L2 error".into()),
                _ => HeartbeatL2Status::Silent,
            };
            Ok(HeartbeatL2Result {
                status,
                duration_ms: 50,
            })
        }
    }

    fn make_task() -> HeartbeatTask {
        HeartbeatTask::new(
            "Test".to_string(),
            "main".to_string(),
            60_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::Always,
            },
        )
    }

    fn make_ctx(adapter_mode: &'static str) -> Arc<TickContext> {
        Arc::new(TickContext {
            probe_executor: Arc::new(MockProbe { result: json!(42) }),
            adapter: Arc::new(MockAdapter {
                status: adapter_mode,
            }),
            delivery: Arc::new(DeliveryEngine::new()),
            dedup: Arc::new(DedupEngine::noop(DedupConfig::default())),
            job_timeout_secs: 120,
            change_emitter: None,
        })
    }

    #[tokio::test]
    async fn tick_triggered_silent() {
        let task = make_task();
        let ctx = make_ctx("silent");
        let result = execute_heartbeat_tick(&task, None, &ctx).await;
        assert_eq!(result.l1_status, "Triggered");
        assert_eq!(result.l2_status.as_deref(), Some("Silent"));
        assert!(result.error.is_none());
    }

    /// The L2 asked for delivery and the task has no target configured, so
    /// nothing reached anyone. The two status columns must agree about that —
    /// this arm used to report `Delivered` beside `NotRequested`.
    #[tokio::test]
    async fn tick_triggered_delivery_no_config() {
        let task = make_task();
        let ctx = make_ctx("delivery");
        let result = execute_heartbeat_tick(&task, None, &ctx).await;
        assert_eq!(result.l1_status, "Triggered");
        assert_eq!(result.l2_status.as_deref(), Some("NotDelivered"));
        assert_eq!(result.delivery_status.as_deref(), Some("NotRequested"));
    }

    /// A configured target that always fails must not be recorded as a
    /// delivery. `l2_status` is derived from the delivery outcome, so the
    /// history row can never claim `Delivered` alongside `NotDelivered`.
    #[tokio::test]
    async fn tick_delivery_failure_is_not_reported_as_delivered() {
        let mut task = make_task();
        // The test engine registers no target kinds, so this webhook fails at
        // dispatch — the URL is never dialled and the test does no network I/O.
        task.delivery_config = Some(crate::tasks::shared::delivery::DeliveryConfig {
            mode: crate::tasks::shared::delivery::DeliveryMode::Primary,
            targets: vec![
                crate::tasks::shared::delivery::DeliveryTargetConfig::Webhook {
                    url: "https://127.0.0.1:1/never".to_string(),
                    method: None,
                    headers: None,
                },
            ],
            fallback_target: None,
        });

        let ctx = make_ctx("delivery");
        let result = execute_heartbeat_tick(&task, None, &ctx).await;

        assert_eq!(result.l1_status, "Triggered");
        assert_ne!(
            result.l2_status.as_deref(),
            Some("Delivered"),
            "a failed delivery must not be recorded as delivered"
        );
        assert_eq!(result.l2_status.as_deref(), Some("NotDelivered"));
        assert_eq!(result.delivery_status.as_deref(), Some("NotDelivered"));
    }

    #[tokio::test]
    async fn tick_triggered_l2_error() {
        let task = make_task();
        let ctx = make_ctx("error");
        let result = execute_heartbeat_tick(&task, None, &ctx).await;
        assert_eq!(result.l1_status, "Triggered");
        assert_eq!(result.l2_status.as_deref(), Some("Error"));
    }

    #[tokio::test]
    async fn tick_skipped_when_not_triggered() {
        let task = HeartbeatTask::new(
            "Test".to_string(),
            "main".to_string(),
            60_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::GreaterThan(100.0),
            },
        );
        let ctx = make_ctx("silent");
        // Probe returns 42, which is NOT > 100, so trigger_condition is false
        let result = execute_heartbeat_tick(&task, None, &ctx).await;
        assert_eq!(result.l1_status, "Skipped");
        assert!(result.l2_status.is_none());
    }

    #[tokio::test]
    async fn tick_forced_by_wake_reason() {
        let task = HeartbeatTask::new(
            "Test".to_string(),
            "main".to_string(),
            60_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::GreaterThan(100.0),
            },
        );
        let ctx = make_ctx("silent");
        // Even though trigger condition is false, wake_reason forces L2
        let result = execute_heartbeat_tick(&task, Some("user requested"), &ctx).await;
        assert_eq!(result.l1_status, "Triggered");
        assert_eq!(result.l2_status.as_deref(), Some("Silent"));
    }

    /// A configured target that refused the payload is a monitoring failure —
    /// it must feed the backoff ladder and the alert gate. A task with no
    /// target asked for nothing, so it stays Ok. This arm was left unmapped
    /// (`None`) when the label was introduced.
    #[test]
    fn not_delivered_splits_on_whether_delivery_was_requested() {
        assert_eq!(
            l2_run_status("NotDelivered", Some("NotRequested")),
            Some(RunStatus::Ok),
            "no target configured means nothing was asked for"
        );
        assert_eq!(
            l2_run_status("NotDelivered", Some("NotDelivered")),
            Some(RunStatus::Error),
            "a configured target that refused the payload is a real failure"
        );
        assert_eq!(l2_run_status("Silent", None), Some(RunStatus::Ok));
        assert_eq!(l2_run_status("Deduped", None), Some(RunStatus::Skipped));
        assert_eq!(l2_run_status("Error", None), Some(RunStatus::Error));
        assert_eq!(l2_run_status("Martian", None), None);
    }

    fn webhook_target() -> crate::tasks::shared::delivery::DeliveryTargetConfig {
        crate::tasks::shared::delivery::DeliveryTargetConfig::Webhook {
            url: "https://127.0.0.1:1/never".to_string(),
            method: None,
            headers: None,
        }
    }

    /// The default fallback mirrors cron's: a task with somewhere to deliver
    /// gets an after=1 alert pointed at that same place, so a monitor that
    /// broke is not silent.
    #[test]
    fn default_alert_is_synthesized_from_the_delivery_target() {
        let mut task = make_task();
        assert!(
            resolve_alert_config(&task, true).is_none(),
            "no delivery target means there is nowhere to alert"
        );

        task.delivery_config = Some(crate::tasks::shared::delivery::DeliveryConfig {
            mode: crate::tasks::shared::delivery::DeliveryMode::Primary,
            targets: vec![webhook_target()],
            fallback_target: None,
        });
        let cfg = resolve_alert_config(&task, true).expect("default alert must be synthesized");
        assert_eq!(cfg.after, 1);
        assert_eq!(cfg.cooldown_ms, 3_600_000);

        assert!(
            resolve_alert_config(&task, false).is_none(),
            "notify_on_failure_default=false must not synthesize"
        );
    }

    /// A failed alert send must not lose the news — the target is often the
    /// same channel whose failure is being reported, so this is the normal
    /// case, not the edge case.
    #[test]
    fn a_parked_alert_rides_along_with_the_next_one() {
        assert_eq!(merge_parked_alert(None, "second".into()), "second");
        let merged = merge_parked_alert(Some("first".into()), "second".into());
        assert!(merged.starts_with("first"));
        assert!(merged.ends_with("second"));
        assert!(merged.contains("previously undelivered"));
    }

    /// An explicit config always wins over the synthesized default.
    #[test]
    fn explicit_alert_config_wins() {
        let mut task = make_task();
        task.delivery_config = Some(crate::tasks::shared::delivery::DeliveryConfig {
            mode: crate::tasks::shared::delivery::DeliveryMode::Primary,
            targets: vec![webhook_target()],
            fallback_target: None,
        });
        task.failure_alert = Some(crate::tasks::shared::alert::FailureAlertConfig {
            after: 7,
            cooldown_ms: 42,
            target: webhook_target(),
        });
        let cfg = resolve_alert_config(&task, true).unwrap();
        assert_eq!(cfg.after, 7);
        assert_eq!(cfg.cooldown_ms, 42);
    }
}

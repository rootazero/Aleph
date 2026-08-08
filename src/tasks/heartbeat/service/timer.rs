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

use crate::tasks::heartbeat::config::HeartbeatTask;
use crate::tasks::heartbeat::dedup::{DedupEngine, DedupVerdict};
use crate::tasks::heartbeat::executor::{
    build_heartbeat_prompt, HeartbeatExecutionAdapter, HeartbeatL2Status,
};
use crate::tasks::heartbeat::history::HeartbeatRunRecord;
use crate::tasks::heartbeat::probe::{execute_probe, ProbeExecutor};
use crate::tasks::heartbeat::service::state::HeartbeatServiceState;
use crate::tasks::heartbeat::wake::{WakeQueue, WakeRequest};
use crate::tasks::shared::delivery::{DeliveryEngine, DeliveryPayload};

// ── TickContext ───────────────────────────────────────────────────────

/// Shared context for each tick execution, holding all external dependencies.
pub struct TickContext {
    pub probe_executor: Arc<dyn ProbeExecutor>,
    pub adapter: Arc<dyn HeartbeatExecutionAdapter>,
    pub delivery: Arc<DeliveryEngine>,
    pub dedup: Arc<DedupEngine>,
    /// Per-task execution timeout (seconds) — bounds both the L1 probe and
    /// the L2 agent turn so a hung tool cannot stall a worker forever.
    pub job_timeout_secs: u64,
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
                writeback_one(&state, &task_id, result).await;
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
                                        let label = if ok { "Delivered" } else { "NotDelivered" };
                                        (ok, Some(label.to_string()))
                                    } else {
                                        (false, Some("NotRequested".to_string()))
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

/// Write a single task's tick result back to the store.
///
/// Called from each task's own detached future, so writebacks for different
/// tasks interleave freely — the store mutex serializes them.
async fn writeback_one(
    state: &HeartbeatServiceState,
    task_id: &str,
    tick_result: HeartbeatTickResult,
) {
    let mut store = state.store.lock().await;
    let now_ms = state.clock.now_ms();

    if let Some(task) = store.get_task_mut(task_id) {
        task.state.running_at_ms = None;
        task.state.last_probe_at_ms = Some(now_ms);

        if let Some(ref pr) = tick_result.new_probe_result {
            task.state.last_probe_result = Some(pr.clone());
        }

        // Update L2 status
        if let Some(ref l2_status) = tick_result.l2_status {
            task.state.last_l2_at_ms = Some(now_ms);
            task.state.last_l2_status = match l2_status.as_str() {
                "Silent" => Some(crate::tasks::cron::config::RunStatus::Ok),
                "Delivered" => Some(crate::tasks::cron::config::RunStatus::Ok),
                "Deduped" => Some(crate::tasks::cron::config::RunStatus::Skipped),
                "Error" => Some(crate::tasks::cron::config::RunStatus::Error),
                // Recognised but deliberately unmapped: whether an undelivered
                // L2 result should count as a task-level failure (and so feed
                // the backoff ladder and the alert chain) is a policy call that
                // belongs with the heartbeat alerting work, not with this
                // label fix. Listed explicitly so it does not masquerade as an
                // unknown status in the log below.
                "NotDelivered" => None,
                unknown => {
                    tracing::warn!(l2_status = %unknown, "unrecognized heartbeat L2 status");
                    None
                }
            };
        }

        let had_error =
            tick_result.error.is_some() || tick_result.l2_status.as_deref() == Some("Error");
        if had_error {
            task.state.consecutive_errors = task.state.consecutive_errors.saturating_add(1);
            task.state.last_error = tick_result.error.clone();
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
            trigger_source: "Interval".to_string(),
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
}

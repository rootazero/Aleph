//! Timer loop and tick execution for heartbeat tasks.
//!
//! The timer loop is the heartbeat of the heartbeat service (meta!). It wakes
//! up periodically or on WakeQueue notification, finds due tasks, runs L1 probes,
//! optionally triggers L2 agent analysis, handles dedup and delivery, and writes
//! back results.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::future::join_all;
use tokio::sync::Semaphore;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info};

use crate::tasks::heartbeat::config::{error_backoff_ms, HeartbeatTask};
use crate::tasks::heartbeat::dedup::DedupEngine;
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
/// 1. Sleep for `tick_interval_secs` OR wake on WakeQueue notification
/// 2. CAS guard against re-entrancy
/// 3. Collect due tasks (by timer or by wake request)
/// 4. Execute tasks concurrently (bounded by semaphore)
/// 5. Write back results
pub async fn run_heartbeat_loop(
    state: Arc<HeartbeatServiceState>,
    wake_queue: Arc<WakeQueue>,
    ctx: Arc<TickContext>,
) {
    let interval = Duration::from_secs(state.config.tick_interval_secs);
    info!(
        tick_interval_secs = state.config.tick_interval_secs,
        "heartbeat timer loop started"
    );

    loop {
        tokio::select! {
            _ = sleep(interval) => {},
            _ = wake_queue.notified() => {},
        }

        if state.is_shutdown() {
            info!("heartbeat timer loop: shutdown requested, exiting");
            break;
        }

        // Atomic CAS for re-entrancy guard
        if state
            .running()
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            debug!("heartbeat timer loop: previous tick still running, skipping");
            continue;
        }

        let wake_requests = wake_queue.drain();
        let due_tasks = collect_due_tasks(&state, &wake_requests).await;

        if !due_tasks.is_empty() {
            debug!(count = due_tasks.len(), "heartbeat tick: found due tasks");

            let semaphore = Arc::new(Semaphore::new(state.config.max_concurrent));
            let mut handles = Vec::with_capacity(due_tasks.len());

            for (task, wake_reason) in due_tasks {
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore should not be closed since we just created it");
                let ctx = ctx.clone();
                let task_id = task.id.clone();
                handles.push(tokio::spawn(async move {
                    let result = execute_heartbeat_tick(&task, wake_reason.as_deref(), &ctx).await;
                    drop(permit);
                    (task_id, result)
                }));
            }

            let results = join_all(handles).await;
            writeback_results(&state, results).await;
        }

        state.running().store(false, Ordering::Release);
    }
}

// ── Collect Due Tasks ────────────────────────────────────────────────

/// Find all tasks that are due by timer or have been woken.
async fn collect_due_tasks(
    state: &HeartbeatServiceState,
    wake_requests: &[WakeRequest],
) -> Vec<(HeartbeatTask, Option<String>)> {
    let store = state.store.lock().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut due = Vec::new();

    for task in store.tasks() {
        if !task.enabled {
            continue;
        }
        // Skip tasks that are already running (per-task exclusion)
        if task.state.running_at_ms.is_some() {
            continue;
        }

        // Check if task is due by timer
        let is_due = task.state.next_due_ms.map(|t| t <= now_ms).unwrap_or(false);

        // Check if task was woken
        let wake_reason = wake_requests
            .iter()
            .find(|w| w.task_id == task.id)
            .and_then(|w| w.reason.clone());

        if is_due || wake_reason.is_some() {
            due.push((task.clone(), wake_reason));
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
    // L1 probe
    let probe_result = execute_probe(
        &task.probe,
        ctx.probe_executor.as_ref(),
        task.state.last_probe_result.as_deref(),
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
                .execute_heartbeat(&task.agent_id, &prompt, 120)
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
                            if ctx.dedup.is_duplicate(&task.id, output).await {
                                ("Deduped".into(), None)
                            } else {
                                let ds = if let Some(ref config) = task.delivery_config {
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
                                    if ok {
                                        Some("Delivered".into())
                                    } else {
                                        Some("NotDelivered".into())
                                    }
                                } else {
                                    Some("NotRequested".into())
                                };
                                ctx.dedup.record(&task.id, output).await;
                                ("Delivered".into(), ds)
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

// ── Writeback Results ────────────────────────────────────────────────

/// Write execution results back to the store.
async fn writeback_results(
    state: &HeartbeatServiceState,
    results: Vec<Result<(String, HeartbeatTickResult), tokio::task::JoinError>>,
) {
    let mut store = state.store.lock().await;
    let now_ms = chrono::Utc::now().timestamp_millis();

    for result in results {
        let Ok((task_id, tick_result)) = result else {
            error!("heartbeat task panicked during execution");
            continue;
        };

        if let Some(task) = store.get_task_mut(&task_id) {
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
                    _ => None,
                };
            }

            let had_error =
                tick_result.error.is_some() || tick_result.l2_status.as_deref() == Some("Error");

            if had_error {
                task.state.consecutive_errors += 1;
                task.state.last_error = tick_result.error.clone();
            } else {
                task.state.consecutive_errors = 0;
                task.state.last_error = None;
            }

            // Recompute next due
            let interval = task.interval_ms as i64;
            let backoff = error_backoff_ms(task.state.consecutive_errors) as i64;
            task.state.next_due_ms = Some(now_ms + interval + backoff);

            // Write history record
            let run_record = HeartbeatRunRecord {
                id: uuid::Uuid::new_v4().to_string(),
                task_id: task_id.clone(),
                trigger_source: "Interval".to_string(),
                l1_status: tick_result.l1_status.clone(),
                l2_status: tick_result.l2_status.clone(),
                started_at: now_ms
                    - tick_result.l1_duration_ms
                    - tick_result.l2_duration_ms.unwrap_or(0),
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

    #[tokio::test]
    async fn tick_triggered_delivery_no_config() {
        let task = make_task();
        let ctx = make_ctx("delivery");
        let result = execute_heartbeat_tick(&task, None, &ctx).await;
        assert_eq!(result.l1_status, "Triggered");
        assert_eq!(result.l2_status.as_deref(), Some("Delivered"));
        assert_eq!(result.delivery_status.as_deref(), Some("NotRequested"));
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

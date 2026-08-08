//! Three-phase concurrency model for cron job execution.
//!
//! - **Phase 1**: Mark due jobs as running (lock, snapshot, persist, release)
//! - **Phase 2**: Execute jobs (handled externally, no lock held)
//! - **Phase 3**: Write back results (lock, reload, update, persist, release)
//!
//! This model minimizes lock hold time — the lock is only held during
//! brief metadata updates, never during actual job execution.

use tracing::{info, warn};

use crate::sync_primitives::Arc;

use crate::tasks::cron::alert::should_send_alert;
use crate::tasks::cron::chain;
use crate::tasks::cron::config::{
    ExecutionResult, JobSnapshot, RunStatus, ScheduleKind, TriggerSource,
};
use crate::tasks::cron::history::CronRunRecord;
use crate::tasks::cron::store::CronStore;
use crate::tasks::cron::template::resolve_job_prompt;
use crate::tasks::shared::clock::Clock;
use crate::tasks::shared::retry_hint::RetryHint;
use crate::tasks::shared::schedule::compute_backoff_ms_for;

use super::ops::{advance_next_run, recompute_next_run_maintenance};

/// Consecutive *transient* failures required before alerting the user.
///
/// Permanent failures (auth, missing agent, validation) alert on the first
/// error — they need operator intervention and won't self-heal. Transient
/// failures (rate-limit / overload / network / timeout / 5xx) are expected to
/// recover on their own, so the scheduler perseveres silently through the
/// backoff ladder and only escalates once they persist across `5` consecutive
/// runs — by which point the windowed-backoff floor has spread those attempts
/// across ≈30 minutes, a strong signal the provider window is genuinely
/// exhausted rather than momentarily throttled. This is what keeps a 8am 429
/// blip from firing a "failed 3 times" alarm.
const TRANSIENT_ALERT_FLOOR: u32 = 5;

/// A pending failure alert returned from [`phase3_writeback`] for the caller to
/// dispatch via the shared delivery pipeline. The phase3 path itself does not
/// hold the channel registry or delivery handle — it only decides *whether* an
/// alert should fire and packages the payload.
#[derive(Debug, Clone)]
pub struct PendingAlert {
    pub job_id: String,
    pub job_name: String,
    pub message: String,
    pub target: crate::tasks::cron::config::DeliveryTargetConfig,
}

/// Phase 1: Mark due jobs and return snapshots for execution.
///
/// Locks the store, finds all enabled jobs that are:
/// - Not already running (`running_at_ms == None`)
/// - Past due (`next_run_at_ms <= now`)
///
/// For each due job, sets `running_at_ms = now`, **advances the schedule to
/// the next future occurrence** (at-most-once), and creates a `JobSnapshot`.
/// Persists and releases the lock.
///
/// `default_timeout_ms` is the configured per-job execution timeout, threaded
/// into each snapshot so it reaches the executor.
pub async fn phase1_mark_due_jobs<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    default_timeout_ms: i64,
) -> Result<Vec<JobSnapshot>, String> {
    let now = clock.now_ms();
    let mut guard = store.lock().await;

    guard.reload_if_changed()?;

    let mut snapshots = Vec::new();

    for job in guard.jobs_mut().iter_mut() {
        if !job.enabled {
            continue;
        }
        if job.state.running_at_ms.is_some() {
            continue;
        }
        match job.state.next_run_at_ms {
            Some(t) if t <= now => {}
            _ => continue,
        }

        // Mark as running. Take the manual-trigger flag in the same breath:
        // whoever set it asked for *this* run, and consuming it here is what
        // keeps `trigger_source` from being a constant (`run_job` can only
        // express "run now" by pulling `next_run_at_ms` forward, which looks
        // exactly like the schedule coming due).
        let manual = std::mem::take(&mut job.state.manual_trigger_pending);
        job.state.running_at_ms = Some(now);

        // At-most-once: advance the schedule to the next future occurrence
        // *before* execution. If the process crashes between here and phase 3
        // writeback, the schedule is already advanced — a recurring job will
        // not re-run the same trigger, and a one-shot `At` job (whose next
        // occurrence is `None`) will not fire again.
        advance_next_run(job, clock);

        snapshots.push(JobSnapshot {
            id: job.id.clone(),
            agent_id: Some(job.agent_id.clone()),
            source_channel_id: job.source_channel_id.clone(),
            source_conversation_id: job.source_conversation_id.clone(),
            prompt: resolve_job_prompt(job, clock),
            model: None,
            timeout_ms: Some(job.timeout_ms.unwrap_or(default_timeout_ms)),
            delivery: job.delivery_config.clone(),
            session_target: job.session_target.clone(),
            marked_at: now,
            trigger_source: if manual {
                TriggerSource::Manual
            } else {
                TriggerSource::Schedule
            },
            owner_user_id: job.owner_user_id.clone(),
            scope_id: job.scope_id.clone(),
        });
    }

    if !snapshots.is_empty() {
        guard.persist()?;
    }

    Ok(snapshots)
}

// `phase1_mark_manual` lived here: it claimed a job, advanced its schedule and
// handed back a snapshot stamped `TriggerSource::Manual`. Nothing in the
// process ever called it — `CronService` holds no executor, so a snapshot it
// produced would have been marked running with no one to run it or to clear
// the marker. Manual triggers instead flow through the one loop that *does*
// have an executor: `run_job` sets `JobStateV2::manual_trigger_pending` and
// `phase1_mark_due_jobs` consumes it. Removed rather than reconnected (R10).

/// Phase 3: Write back execution results after jobs complete.
///
/// Locks the store, force-reloads to capture concurrent edits,
/// then for each result:
/// - Clears `running_at_ms`
/// - Writes execution result fields (status, error, duration, etc.)
/// - Resets or increments `consecutive_errors`
///
/// `next_run_at_ms` is **not** cleared here: phase 1 already advanced the
/// schedule before execution (at-most-once). After processing all results
/// this function:
/// - removes one-shot `At` jobs flagged `delete_after_run`,
/// - runs maintenance recompute (fills only still-missing `next_run_at_ms`),
/// - applies failure backoff to recurring jobs that just failed,
/// - fires job chains,
/// - persists.
pub async fn phase3_writeback<C: Clock>(
    store: &Arc<tokio::sync::Mutex<CronStore>>,
    clock: &C,
    results: &[(String, ExecutionResult)],
    notify_on_failure_default: bool,
) -> Result<Vec<PendingAlert>, String> {
    let mut guard = store.lock().await;

    guard.force_reload()?;

    // (source_job_id, successor_job_id) pairs resolved during the result
    // loop; fired after maintenance recompute so the `job` borrow is free.
    let mut chain_triggers: Vec<(String, String)> = Vec::new();

    // One-shot `At` jobs flagged `delete_after_run`, removed after history
    // is recorded so the run is preserved.
    let mut jobs_to_delete: Vec<String> = Vec::new();

    for (job_id, result) in results {
        let job = match guard.get_job_mut(job_id) {
            Some(j) => j,
            None => {
                warn!("phase3: job '{job_id}' was deleted during execution, discarding result");
                continue;
            }
        };

        // Clear running state
        job.state.running_at_ms = None;

        // Write execution result
        job.state.last_run_at_ms = Some(result.started_at);
        job.state.last_run_status = Some(result.status);
        job.state.last_duration_ms = Some(result.duration_ms);
        job.state.last_error = result.error.clone();
        job.state.last_error_reason = result.error_reason.clone();
        job.state.last_delivery_status = result.delivery_status;

        // Template context for the next run: a bounded copy of this run's
        // output and an incremented run counter feed `{{last_output}}` /
        // `{{run_count}}` when the job uses a `prompt_template`.
        job.state.run_count = job.state.run_count.saturating_add(1);
        // Only overwrite `last_output` when this run actually produced output.
        // A Skipped/Error/Timeout run carries `output: None`; nulling the prior
        // value would wipe the accumulated `{{last_output}}` self-chain context
        // even though the run never executed.
        if let Some(s) = result.output.as_ref() {
            const MAX_CHARS: usize = 2_000;
            job.state.last_output = Some(if s.chars().count() > MAX_CHARS {
                s.chars().take(MAX_CHARS).collect()
            } else {
                s.clone()
            });
        }

        // Update consecutive errors
        match result.status {
            RunStatus::Ok | RunStatus::Skipped => {
                job.state.consecutive_errors = 0;
            }
            RunStatus::Error | RunStatus::Timeout => {
                job.state.consecutive_errors = job.state.consecutive_errors.saturating_add(1);
            }
        }

        // Resolve the chain successor for this outcome. Triggering needs
        // `&mut store`, so collect here and apply after the `job` borrow ends.
        let chain_target = match result.status {
            RunStatus::Ok | RunStatus::Skipped => job.next_job_id_on_success.clone(),
            RunStatus::Error | RunStatus::Timeout => job.next_job_id_on_failure.clone(),
        };
        if let Some(target) = chain_target {
            chain_triggers.push((job_id.clone(), target));
        }

        // One-shot `At` jobs with `delete_after_run` are removed once their
        // *appointment* has run. `next_run_at_ms` is left as phase 1 set it
        // (recurring jobs keep their advanced schedule; a fired `At` job is
        // already `None`).
        //
        // Manual runs are excluded: "run it now to see what it does" must not
        // consume a reminder that has not come due yet. Previously it did —
        // the job vanished, its future firing with it, and the only trace was
        // a history row.
        if matches!(
            job.schedule_kind,
            ScheduleKind::At {
                delete_after_run: true,
                ..
            }
        ) && matches!(result.trigger_source, TriggerSource::Schedule)
        {
            jobs_to_delete.push(job_id.clone());
        }
    }

    // Record execution history (now also persists retry hint classification)
    for (job_id, result) in results {
        let (retry_category, retryable) = match result.retry_hint {
            Some(hint) => (
                hint.category.map(|c| c.as_str().to_string()),
                Some(hint.retryable),
            ),
            None => (None, None),
        };
        let record = CronRunRecord {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.clone(),
            trigger_source: result.trigger_source.as_str().to_string(),
            status: format!("{:?}", result.status).to_lowercase(),
            started_at: result.started_at,
            ended_at: Some(result.ended_at),
            duration_ms: Some(result.duration_ms),
            error: result.error.clone(),
            error_reason: result
                .error_reason
                .as_ref()
                .map(|r| r.category().to_string()),
            output_summary: None,
            delivery_status: result
                .delivery_status
                .map(|s| format!("{s:?}").to_lowercase()),
            created_at: clock.now_ms(),
            retry_category,
            retryable,
        };
        if let Err(e) = guard.insert_run(&record) {
            warn!(job_id, error = %e, "failed to record cron run history");
        }
    }

    // Remove one-shot jobs flagged `delete_after_run` (history is recorded).
    for job_id in &jobs_to_delete {
        guard.remove_job(job_id);
    }

    // Maintenance recompute ALL jobs (fills only still-missing next_run_at_ms).
    {
        let jobs = guard.jobs_mut();
        for job in jobs.iter_mut() {
            recompute_next_run_maintenance(job, clock);
        }
    }

    // Failure backoff + retry-hint short-circuit:
    //   - **Transient** failures (rate_limit / overloaded / network / timeout /
    //     server_error) keep the existing escalating backoff so we don't
    //     hot-loop a recurring job at its normal interval.
    //   - **Permanent** failures (auth error, missing agent, validation) skip
    //     scheduling another retry entirely — `next_run_at_ms` is cleared so
    //     the operator gets the alert below and can fix the config without the
    //     job churning in the background.
    //
    // Both cases stay limited to non-`At` (recurring) jobs; one-shot `At` jobs
    // are never rescheduled and may have already been removed above.
    let backoff_now = clock.now_ms();
    for (job_id, result) in results {
        if !matches!(result.status, RunStatus::Error | RunStatus::Timeout) {
            continue;
        }
        let Some(job) = guard.get_job_mut(job_id) else {
            continue;
        };
        if matches!(job.schedule_kind, ScheduleKind::At { .. }) {
            continue;
        }
        let hint = result.retry_hint.unwrap_or_else(RetryHint::permanent);
        if !hint.retryable {
            // Permanent: do not schedule another run; the alert dispatcher
            // below will notify the user with the classified reason.
            if job.state.next_run_at_ms.is_some() {
                info!(
                    job_id,
                    "phase3: permanent failure detected, clearing next_run_at_ms"
                );
            }
            job.state.next_run_at_ms = None;
            continue;
        }
        let backoff = compute_backoff_ms_for(hint.category, job.state.consecutive_errors);
        if backoff > 0 {
            let floor = backoff_now.saturating_add(backoff);
            let next = job.state.next_run_at_ms.map_or(floor, |n| n.max(floor));
            job.state.next_run_at_ms = Some(next);
        }
    }

    // Failure alerts: for any job configured with `failure_alert`, evaluate
    // `should_send_alert` against the freshly-incremented `consecutive_errors`
    // and the run's classified retry hint. Permanent failures bypass the
    // cooldown (the original alert helper only respected `cooldown_ms`, so the
    // policy lives here in the caller). The returned `PendingAlert`s are
    // dispatched by the timer loop via the shared delivery pipeline — phase3
    // keeps no I/O coupling beyond SQLite.
    let now_alert = clock.now_ms();
    let mut pending_alerts: Vec<PendingAlert> = Vec::new();
    for (job_id, result) in results {
        if !matches!(result.status, RunStatus::Error | RunStatus::Timeout) {
            continue;
        }
        let Some(job) = guard.get_job_mut(job_id) else {
            continue;
        };
        // D4: when no explicit `failure_alert` is configured but the job has
        // a delivery channel + conversation, synthesize a sane default so
        // failures aren't silently dropped (which previously required users
        // to opt in to alerting per job).
        let alert_cfg = job.failure_alert.clone().or_else(|| {
            if !notify_on_failure_default {
                return None;
            }
            let channel = job.source_channel_id.clone()?;
            let chat_id = job.source_conversation_id.clone()?;
            Some(crate::tasks::cron::config::FailureAlertConfig {
                after: 1,
                cooldown_ms: 3_600_000,
                target: crate::tasks::cron::config::DeliveryTargetConfig::Gateway {
                    channel,
                    chat_id,
                    format: None,
                },
            })
        });
        let Some(alert_cfg) = alert_cfg else {
            continue;
        };
        let hint = result.retry_hint.unwrap_or_else(RetryHint::permanent);
        // Permanent failures fire on the first error, bypassing both the
        // `after` threshold and the cooldown — operator intervention is
        // required anyway, no point burying it under retry noise.
        let bypass = !hint.retryable;
        let msg = if bypass {
            Some(format!(
                "Cron job '{}' ({}) failed permanently (no further retries scheduled). \
                 Last error: {}",
                job.name,
                job.id,
                job.state.last_error.as_deref().unwrap_or("unknown")
            ))
        } else if job.state.consecutive_errors < TRANSIENT_ALERT_FLOOR {
            // Transient failure still within the perseverance window — keep
            // retrying with backoff, don't alarm the user over a self-healing
            // throttle. (A momentary rate-limit that succeeds on the next
            // attempt resets `consecutive_errors` to 0 and never reaches here.)
            None
        } else {
            should_send_alert(job, &alert_cfg, now_alert)
        };
        if let Some(message) = msg {
            job.state.last_failure_alert_at_ms = Some(now_alert);
            pending_alerts.push(PendingAlert {
                job_id: job.id.clone(),
                job_name: job.name.clone(),
                message,
                target: alert_cfg.target,
            });
        }
    }

    // Fire job chains: enqueue each successor to run on the next tick.
    // Done *after* maintenance recompute so the chain trigger's
    // `next_run_at_ms = now` wins over a recomputed schedule. Cyclic chains
    // are skipped — chain.rs has no creation-time guard, so an A->B->A loop
    // would otherwise re-trigger every tick (runaway).
    let chain_now = clock.now_ms();
    for (source_id, target_id) in chain_triggers {
        match chain::detect_cycle(&guard, &source_id, &target_id) {
            Ok(true) => {
                warn!(
                    source = %source_id,
                    target = %target_id,
                    "phase3: skipping cyclic chain trigger"
                );
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                warn!(
                    source = %source_id,
                    target = %target_id,
                    error = %e,
                    "phase3: chain cycle check failed; skipping trigger"
                );
                continue;
            }
        }
        match chain::trigger_chain_job(&mut guard, &target_id, chain_now) {
            // Triggered, or a benign no-op (target missing or disabled).
            Ok(_) => {}
            Err(e) => warn!(
                target = %target_id,
                error = %e,
                "phase3: chain trigger failed"
            ),
        }
    }

    guard.persist()?;

    Ok(pending_alerts)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::{CronJob, RunStatus, ScheduleKind};
    use crate::tasks::cron::service::ops::add_job;
    use crate::tasks::shared::clock::testing::FakeClock;
    use tempfile::TempDir;

    /// Configured per-job timeout used by the test timer.
    const TEST_TIMEOUT_MS: i64 = 300_000;

    fn make_test_job(id: &str) -> CronJob {
        let mut job = CronJob::new(
            id.to_string(),
            "test-agent".to_string(),
            "test prompt".to_string(),
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.id = id.to_string();
        job
    }

    fn make_store() -> (Arc<tokio::sync::Mutex<CronStore>>, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.db");
        let store = CronStore::load(path).unwrap();
        (Arc::new(tokio::sync::Mutex::new(store)), dir)
    }

    fn make_execution_result(status: RunStatus) -> ExecutionResult {
        // Failed-status fixtures default to a transient retry hint so existing
        // tests that exercise the *backoff* path keep working — without this
        // a `None` hint trips the permanent short-circuit added by the
        // retry-hint port and `next_run_at_ms` is cleared. Tests that
        // specifically want the permanent branch override the hint after.
        let retry_hint = if matches!(status, RunStatus::Error | RunStatus::Timeout) {
            Some(crate::tasks::shared::retry_hint::RetryHint::transient(
                crate::tasks::shared::retry_hint::RetryCategory::Timeout,
            ))
        } else {
            None
        };
        ExecutionResult {
            started_at: 1_000_000,
            ended_at: 1_005_000,
            duration_ms: 5_000,
            status,
            output: Some("done".to_string()),
            error: if status == RunStatus::Error {
                Some("test error".to_string())
            } else {
                None
            },
            error_reason: None,
            delivery_status: None,
            agent_used_messaging_tool: false,
            trigger_source: TriggerSource::Schedule,
            retry_hint,
        }
    }

    #[tokio::test]
    async fn phase1_marks_due_jobs() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        // Add a job with next_run in the past (due)
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("due-job");
            job.created_at = 900_000;
            let id = add_job(&mut guard, job, &clock);
            // Manually set next_run to past
            let j = guard.get_job_mut(&id).unwrap();
            j.state.next_run_at_ms = Some(950_000);
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "due-job");
        assert_eq!(snapshots[0].trigger_source, TriggerSource::Schedule);

        // Verify running_at_ms was set
        let guard = store.lock().await;
        let job = guard.get_job("due-job").unwrap();
        assert_eq!(job.state.running_at_ms, Some(1_000_000));
    }

    #[tokio::test]
    async fn phase1_skips_already_running() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("running-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("running-job").unwrap();
            j.state.next_run_at_ms = Some(950_000);
            j.state.running_at_ms = Some(990_000); // already running
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert!(snapshots.is_empty(), "running jobs should be skipped");
    }

    #[tokio::test]
    async fn phase3_merges_results() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        // Set up a running job
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("completed-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("completed-job").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.consecutive_errors = 3; // had errors before
            guard.persist().unwrap();
        }

        let result = make_execution_result(RunStatus::Ok);
        let results = vec![("completed-job".to_string(), result)];

        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("completed-job").unwrap();
        assert!(
            job.state.running_at_ms.is_none(),
            "running_at_ms should be cleared"
        );
        assert_eq!(job.state.last_run_status, Some(RunStatus::Ok));
        assert_eq!(job.state.consecutive_errors, 0, "errors should reset on Ok");
        assert!(
            job.state.next_run_at_ms.is_some(),
            "next_run should be recomputed by maintenance"
        );
    }

    #[tokio::test]
    async fn phase3_handles_deleted_job() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        // Don't add any job — simulate deletion during execution
        let result = make_execution_result(RunStatus::Ok);
        let results = vec![("deleted-job".to_string(), result)];

        // Should not error, just warn
        let outcome = phase3_writeback(&store, &clock, &results, false).await;
        assert!(outcome.is_ok(), "should gracefully handle deleted jobs");
    }

    #[tokio::test]
    async fn phase3_increments_consecutive_errors() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);

        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("error-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("error-job").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.consecutive_errors = 2; // had 2 errors
            guard.persist().unwrap();
        }

        let result = make_execution_result(RunStatus::Error);
        let results = vec![("error-job".to_string(), result)];

        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("error-job").unwrap();
        assert_eq!(
            job.state.consecutive_errors, 3,
            "consecutive_errors should increment on Error"
        );
    }

    #[tokio::test]
    async fn phase3_triggers_success_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(1_100_000),
            "Ok result triggers next_job_id_on_success to run now"
        );
    }

    #[tokio::test]
    async fn phase3_triggers_failure_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_failure = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(1_100_000),
            "Error result triggers next_job_id_on_failure to run now"
        );
    }

    #[tokio::test]
    async fn phase3_success_chain_not_fired_on_failure() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "Error result must not fire the on_success chain"
        );
    }

    #[tokio::test]
    async fn phase3_skips_cyclic_chain() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            let mut job_b = make_test_job("job-b");
            job_b.next_job_id_on_success = Some("job-a".to_string());
            add_job(&mut guard, job_b, &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "a cyclic chain (a->b->a) must not be triggered"
        );
    }

    #[tokio::test]
    async fn phase3_chain_target_disabled_not_triggered() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("job-b".to_string());
            add_job(&mut guard, job_a, &clock);
            add_job(&mut guard, make_test_job("job-b"), &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            let b = guard.get_job_mut("job-b").unwrap();
            b.enabled = false;
            b.state.next_run_at_ms = Some(9_999_999_999);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let b = guard.get_job("job-b").unwrap();
        assert_eq!(
            b.state.next_run_at_ms,
            Some(9_999_999_999),
            "a disabled chain target must not be triggered"
        );
    }

    #[tokio::test]
    async fn phase3_chain_target_missing_is_noop() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job_a = make_test_job("job-a");
            job_a.next_job_id_on_success = Some("ghost".to_string());
            add_job(&mut guard, job_a, &clock);
            let a = guard.get_job_mut("job-a").unwrap();
            a.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("job-a".to_string(), make_execution_result(RunStatus::Ok))];
        let outcome = phase3_writeback(&store, &clock, &results, false).await;
        assert!(
            outcome.is_ok(),
            "a chain target that no longer exists must not error"
        );
    }

    /// The manual-trigger flag is what makes `trigger_source` mean anything:
    /// without it every snapshot the timer produces says `Schedule`, and the
    /// phase-3 gate that reads it is a constant.
    #[tokio::test]
    async fn phase1_stamps_manual_trigger_and_clears_the_flag() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("manual-job");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("manual-job").unwrap();
            j.state.next_run_at_ms = Some(950_000); // due
            j.state.manual_trigger_pending = true; // as `run_job` leaves it
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].trigger_source, TriggerSource::Manual);

        // One-shot: the flag must not leak into the next scheduled run.
        let guard = store.lock().await;
        assert!(
            !guard
                .get_job("manual-job")
                .unwrap()
                .state
                .manual_trigger_pending,
            "the manual flag is consumed by the run it triggered"
        );
    }

    /// C1: phase 1 advances a recurring job's schedule *before* execution,
    /// so a crash before phase 3 does not double-run the same trigger.
    #[tokio::test]
    async fn phase1_advances_recurring_schedule_before_execution() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("recurring");
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("recurring").unwrap();
            j.state.next_run_at_ms = Some(950_000); // due
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].timeout_ms,
            Some(TEST_TIMEOUT_MS),
            "configured timeout must reach the snapshot"
        );

        let guard = store.lock().await;
        let job = guard.get_job("recurring").unwrap();
        let next = job
            .state
            .next_run_at_ms
            .expect("recurring job keeps a next run");
        assert!(
            next > 1_000_000,
            "next_run must be advanced past now before execution, got {next}"
        );
    }

    /// C1: phase 1 clears a one-shot `At` job's schedule before execution,
    /// so it cannot fire twice even across a crash.
    #[tokio::test]
    async fn phase1_clears_one_shot_schedule_before_execution() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "once",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 950_000,
                    delete_after_run: false,
                },
            );
            job.id = "once".to_string();
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("once").unwrap();
            j.state.next_run_at_ms = Some(950_000); // due
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);

        let guard = store.lock().await;
        let job = guard.get_job("once").unwrap();
        assert_eq!(
            job.state.next_run_at_ms, None,
            "one-shot job must not be re-fireable after being marked"
        );
    }

    /// C3: phase 3 removes a one-shot job flagged `delete_after_run`.
    #[tokio::test]
    async fn phase3_deletes_one_shot_with_delete_after_run() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "oneshot",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 1_000_000,
                    delete_after_run: true,
                },
            );
            job.id = "oneshot".to_string();
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("oneshot").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("oneshot".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        assert!(
            guard.get_job("oneshot").is_none(),
            "delete_after_run job should be removed after it runs"
        );
        // History is still recorded.
        let runs = guard.get_runs("oneshot", 10).unwrap();
        assert_eq!(runs.len(), 1, "the run must be preserved in history");
    }

    /// C3: a one-shot job without `delete_after_run` is kept (and inert).
    #[tokio::test]
    async fn phase3_keeps_one_shot_without_delete_after_run() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = CronJob::new(
                "keep",
                "agent",
                "prompt",
                ScheduleKind::At {
                    at: 1_000_000,
                    delete_after_run: false,
                },
            );
            job.id = "keep".to_string();
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("keep").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let results = vec![("keep".to_string(), make_execution_result(RunStatus::Ok))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("keep").expect("job should be kept");
        assert_eq!(
            job.state.next_run_at_ms, None,
            "a completed one-shot job must not re-fire"
        );
    }

    /// C4: a recurring job that fails has its next run pushed out by backoff.
    #[tokio::test]
    async fn phase3_applies_backoff_on_failure() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("flaky"); // Every 60s
            job.created_at = 900_000;
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("flaky").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.next_run_at_ms = Some(1_120_000); // phase1-advanced, soon
            j.state.consecutive_errors = 2; // becomes 3 after this error
            guard.persist().unwrap();
        }

        let results = vec![("flaky".to_string(), make_execution_result(RunStatus::Error))];
        phase3_writeback(&store, &clock, &results, false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("flaky").unwrap();
        // 3 consecutive errors → 5 min backoff tier.
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next >= 1_100_000 + 300_000,
            "backoff not applied: next={next}, expected >= {}",
            1_100_000 + 300_000
        );
    }

    /// D1: per-job `timeout_ms` override beats the configured default
    /// when set; when `None`, the snapshot inherits the default.
    #[tokio::test]
    async fn phase1_per_job_timeout_override_beats_default() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);

        // Job A overrides timeout to 10 minutes; Job B inherits default.
        {
            let mut guard = store.lock().await;

            let mut job_a = make_test_job("override-job");
            job_a.created_at = 900_000;
            job_a.timeout_ms = Some(600_000);
            add_job(&mut guard, job_a, &clock);
            let ja = guard.get_job_mut("override-job").unwrap();
            ja.state.next_run_at_ms = Some(950_000);

            let mut job_b = make_test_job("default-job");
            job_b.created_at = 900_000;
            add_job(&mut guard, job_b, &clock);
            let jb = guard.get_job_mut("default-job").unwrap();
            jb.state.next_run_at_ms = Some(950_000);

            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 2);

        let by_id: std::collections::HashMap<_, _> = snapshots
            .iter()
            .map(|s| (s.id.clone(), s.timeout_ms))
            .collect();
        assert_eq!(
            by_id.get("override-job"),
            Some(&Some(600_000)),
            "per-job override must reach the snapshot"
        );
        assert_eq!(
            by_id.get("default-job"),
            Some(&Some(TEST_TIMEOUT_MS)),
            "jobs without override inherit the configured default"
        );
    }

    /// D4: when `notify_on_failure_default` is true and the job has a
    /// source channel + conversation but no explicit `failure_alert`,
    /// phase3 synthesizes a Gateway-target alert from those fields.
    #[tokio::test]
    async fn phase3_synthesizes_default_alert_for_channel_jobs() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("silent-failer");
            job.source_channel_id = Some("AlephzBot".to_string());
            job.source_conversation_id = Some("123456".to_string());
            assert!(job.failure_alert.is_none());
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("silent-failer").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let result = ExecutionResult {
            status: RunStatus::Timeout,
            error: Some("timeout".to_string()),
            // Permanent hint to bypass the `after` threshold for this test —
            // the assertion is about *synthesis*, not the cooldown policy.
            retry_hint: Some(crate::tasks::shared::retry_hint::RetryHint::permanent()),
            ..make_execution_result(RunStatus::Timeout)
        };
        let alerts = phase3_writeback(
            &store,
            &clock,
            &[("silent-failer".to_string(), result)],
            true, // notify_on_failure_default
        )
        .await
        .unwrap();

        assert_eq!(alerts.len(), 1, "default alert must be synthesized");
        let alert = &alerts[0];
        assert_eq!(alert.job_id, "silent-failer");
        match &alert.target {
            crate::tasks::cron::config::DeliveryTargetConfig::Gateway {
                channel, chat_id, ..
            } => {
                assert_eq!(channel, "AlephzBot");
                assert_eq!(chat_id, "123456");
            }
            other => panic!("expected Gateway target, got {other:?}"),
        }
    }

    /// D4: when `notify_on_failure_default` is false, no alert is synthesized
    /// even if the job has a source channel — preserves legacy opt-in-only.
    #[tokio::test]
    async fn phase3_default_alert_off_preserves_silent_behaviour() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("silent-by-config");
            job.source_channel_id = Some("AlephzBot".to_string());
            job.source_conversation_id = Some("123456".to_string());
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("silent-by-config").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let result = ExecutionResult {
            status: RunStatus::Timeout,
            retry_hint: Some(crate::tasks::shared::retry_hint::RetryHint::permanent()),
            ..make_execution_result(RunStatus::Timeout)
        };
        let alerts = phase3_writeback(
            &store,
            &clock,
            &[("silent-by-config".to_string(), result)],
            false,
        )
        .await
        .unwrap();
        assert!(
            alerts.is_empty(),
            "notify_on_failure_default=false must not synthesize"
        );
    }

    /// D4: jobs without source_channel never get a synthesized default
    /// (no channel to deliver to).
    #[tokio::test]
    async fn phase3_default_alert_requires_source_channel() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let job = make_test_job("no-channel");
            assert!(job.source_channel_id.is_none());
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("no-channel").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            guard.persist().unwrap();
        }

        let result = ExecutionResult {
            status: RunStatus::Error,
            retry_hint: Some(crate::tasks::shared::retry_hint::RetryHint::permanent()),
            ..make_execution_result(RunStatus::Error)
        };
        let alerts = phase3_writeback(&store, &clock, &[("no-channel".to_string(), result)], true)
            .await
            .unwrap();
        assert!(
            alerts.is_empty(),
            "no synthesis without source_channel_id+conversation_id"
        );
    }

    /// Build a failed result whose retry hint is a transient rate-limit —
    /// the exact shape produced when Anthropic returns HTTP 429.
    fn rate_limited_result() -> ExecutionResult {
        ExecutionResult {
            status: RunStatus::Error,
            error: Some("Rate limit error: Anthropic API rate limited (429)".to_string()),
            retry_hint: Some(crate::tasks::shared::retry_hint::RetryHint::transient(
                crate::tasks::shared::retry_hint::RetryCategory::RateLimit,
            )),
            ..make_execution_result(RunStatus::Error)
        }
    }

    /// Regression (user-reported): a momentary 8am 429 must NOT fire a
    /// "failed N times" alert while still inside the transient perseverance
    /// window, and its retry must be floored to the 5-minute windowed backoff
    /// rather than the eager 30s/60s tiers that re-hit the same limit.
    #[tokio::test]
    async fn phase3_rate_limit_below_floor_does_not_alert_and_floors_backoff() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("news-summary"); // Every 60s (recurring)
            job.source_channel_id = Some("AlephzBot".to_string());
            job.source_conversation_id = Some("123456".to_string());
            assert!(job.failure_alert.is_none());
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("news-summary").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.next_run_at_ms = Some(1_120_000); // phase1-advanced, soon
            j.state.consecutive_errors = 2; // becomes 3 after this error
            guard.persist().unwrap();
        }

        let results = vec![("news-summary".to_string(), rate_limited_result())];
        // notify_on_failure_default = true → would synthesize an after:1 alert,
        // but the transient floor must suppress it at 3 consecutive errors.
        let alerts = phase3_writeback(&store, &clock, &results, true)
            .await
            .unwrap();

        assert!(
            alerts.is_empty(),
            "a transient 429 below the floor must not alarm the user (got {alerts:?})"
        );

        let guard = store.lock().await;
        let job = guard.get_job("news-summary").unwrap();
        assert_eq!(job.state.consecutive_errors, 3);
        let next = job.state.next_run_at_ms.unwrap();
        assert!(
            next >= 1_100_000 + 300_000,
            "rate-limit backoff must be floored to 5 min, got next={next}"
        );
    }

    /// Complement: once transient failures persist past the floor (provider
    /// window genuinely exhausted), the alert fires so the operator finds out.
    #[tokio::test]
    async fn phase3_rate_limit_at_floor_does_alert() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_100_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("news-summary");
            job.source_channel_id = Some("AlephzBot".to_string());
            job.source_conversation_id = Some("123456".to_string());
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("news-summary").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            j.state.consecutive_errors = 4; // becomes 5 == TRANSIENT_ALERT_FLOOR
            guard.persist().unwrap();
        }

        let results = vec![("news-summary".to_string(), rate_limited_result())];
        let alerts = phase3_writeback(&store, &clock, &results, true)
            .await
            .unwrap();

        assert_eq!(
            alerts.len(),
            1,
            "sustained transient failure at the floor must alert"
        );
        assert!(alerts[0].message.contains("5 times"));
    }

    /// D1: per-job override also applies on the manual-trigger path.
    #[tokio::test]
    async fn phase1_manual_honours_per_job_timeout_override() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("manual-override");
            job.created_at = 900_000;
            job.timeout_ms = Some(900_000);
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("manual-override").unwrap();
            j.state.next_run_at_ms = Some(950_000);
            j.state.manual_trigger_pending = true;
            guard.persist().unwrap();
        }

        let snapshots = phase1_mark_due_jobs(&store, &clock, TEST_TIMEOUT_MS)
            .await
            .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].trigger_source, TriggerSource::Manual);
        assert_eq!(snapshots[0].timeout_ms, Some(900_000));
    }

    /// A `delete_after_run` one-shot run by hand *before* its appointment must
    /// survive: the manual run is a preview, not the appointment. Before the
    /// trigger-source gate, phase 3 deleted it and the scheduled firing went
    /// with it.
    #[tokio::test]
    async fn phase3_keeps_one_shot_when_the_run_was_manual() {
        let (store, _dir) = make_store();
        let clock = FakeClock::new(1_000_000);
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("reminder");
            job.schedule_kind = ScheduleKind::At {
                at: 2_000_000, // still in the future
                delete_after_run: true,
            };
            add_job(&mut guard, job, &clock);
            let j = guard.get_job_mut("reminder").unwrap();
            j.state.running_at_ms = Some(1_000_000);
            // As phase 1 leaves it: the appointment is still ahead, so
            // `advance_next_run` kept it.
            j.state.next_run_at_ms = Some(2_000_000);
            guard.persist().unwrap();
        }

        let mut result = make_execution_result(RunStatus::Ok);
        result.trigger_source = TriggerSource::Manual;
        phase3_writeback(&store, &clock, &[("reminder".to_string(), result)], false)
            .await
            .unwrap();

        let guard = store.lock().await;
        let job = guard
            .get_job("reminder")
            .expect("a manual preview must not consume the appointment");
        assert_eq!(
            job.state.next_run_at_ms,
            Some(2_000_000),
            "the scheduled firing must still be pending"
        );
    }
}

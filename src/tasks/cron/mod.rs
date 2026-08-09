//! Cron Job Scheduling Service
//!
//! Provides scheduled job execution for automating agent tasks.
//!
//! # Features
//!
//! - Rich schedule kinds: cron expressions, intervals, one-shot
//! - JSON file persistence with atomic writes
//! - Concurrent job execution with configurable limits
//! - Job history and run logs
//! - Failure alerting and delivery pipeline
//! - Job chaining (`on_success` / `on_failure` triggers)
//! - Template rendering with variable substitution
//!
//! # Architecture
//!
//! - `config` — Type definitions (`CronJob`, `ScheduleKind`, `JobStateV2`, etc.)
//! - `store` — JSON atomic persistence
//! - `clock` — Time abstraction for testability
//! - `schedule` — Pure scheduling computation
//! - `stagger` — Hash-based stagger for cron jobs
//! - `service/` — State container, CRUD ops, timer loop, concurrency, catchup
//! - `alert` — Failure alerting
//! - `delivery` — Result delivery pipeline
//! - `chain` — Job chaining with cycle detection
//! - `template` — Prompt template rendering
//! - `webhook_target` — Webhook delivery target

pub mod alert;
pub mod carryover;
pub mod chain;
pub mod config;
pub mod executor;
pub mod history;
pub mod service;
pub mod stagger;
pub mod store;
pub mod template;
pub mod webhook_target;

// Re-export shared infrastructure for backward compatibility
pub use crate::tasks::shared::clock;
pub use crate::tasks::shared::delivery;
pub use crate::tasks::shared::schedule;

// ── Re-exports ──────────────────────────────────────────────────────

pub use crate::tasks::shared::delivery::{DeliveryEngine, DeliveryPayload, DeliveryTarget};
pub use config::{
    CronConfig, CronJob, CronJobView, DeliveryConfig, DeliveryMode, DeliveryOutcome,
    DeliveryStatus, DeliveryTargetConfig, ErrorReason, ExecutionResult, FailureAlertConfig,
    JobSnapshot, JobStateV2, RunStatus, ScheduleKind, SessionTarget, TriggerSource,
};

use crate::sync_primitives::Arc;
use clock::{Clock, SystemClock};
use service::ServiceState;
use store::CronStore;

/// Shared handle to `CronService` for use in gateway handlers
pub type SharedCronService = Arc<tokio::sync::Mutex<CronService>>;

/// High-level cron service wrapping the internal `ServiceState`.
///
/// Provides a simple async API for gateway handlers and CLI.
pub struct CronService {
    state: Arc<ServiceState<SystemClock>>,
    /// Optional event-bus handle. When set, every successful write
    /// (`add_job`, `update_job`, `delete_job`, toggle, run) publishes a
    /// [`crate::gateway::events::GatewayEventFrame::CronJobChanged`] frame so
    /// the panel can drop polling. Wired at startup via [`with_event_bus`].
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl CronService {
    /// Create a new `CronService` from configuration.
    ///
    /// Opens (or creates) the `SQLite` store and initializes the service state.
    pub fn new(config: CronConfig) -> Result<Self, String> {
        config
            .validate()
            .map_err(|e| format!("invalid config: {e}"))?;

        let db_path = config.expand_db_path();
        let path = std::path::PathBuf::from(&db_path);

        // One-time rescue of a store left at the pre-`ALEPH_HOME` location.
        // Must run before `load()`, which would otherwise create an empty
        // database at the new path and make the legacy one unreachable.
        store::migrate_legacy_store(config.legacy_db_path().as_deref(), &path);

        let store = CronStore::load(path)?;
        let clock = Arc::new(SystemClock);
        let store = Arc::new(tokio::sync::Mutex::new(store));
        let state = Arc::new(ServiceState::new(store, clock, config));

        // Spawn the per-process carry-over sweeper exactly once.
        // Guarded by `OnceLock` inside the helper so repeated
        // construction (tests, hot-reload) never double-spawns.
        // No-op when called outside a Tokio runtime.
        carryover::bootstrap_global_carryover_sweeper();

        Ok(Self {
            state,
            event_bus: None,
        })
    }

    /// Attach an event bus so subsequent mutations emit `CronJobChanged`.
    ///
    /// Builder-style setter — services are constructed once at startup and the
    /// bus is wired in immediately after; callers that don't want push (tests,
    /// CLI tooling) can simply skip this.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Internal helper — publishes a `CronJobChanged` frame if the event bus
    /// is wired. Errors are logged at debug level since failures here are
    /// not user-facing.
    fn emit_change(&self, job_id: &str, change: crate::gateway::events::ChangeKind) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::CronJobChanged {
                job_id: job_id.to_string(),
                change,
            };
            if let Err(e) = bus.publish_frame(&frame) {
                tracing::debug!(error = %e, "cron: failed to publish CronJobChanged frame");
            }
        }
    }

    // ── Read operations ─────────────────────────────────────────────

    /// List all jobs as read-only views.
    pub async fn list_jobs(&self) -> Result<Vec<CronJobView>, String> {
        let store = self.state.store.lock().await;
        Ok(service::ops::list_jobs(&store))
    }

    /// Get a single job by ID as a read-only view.
    pub async fn get_job(&self, id: &str) -> Result<CronJobView, String> {
        let store = self.state.store.lock().await;
        service::ops::get_job(&store, id).ok_or_else(|| format!("job not found: {id}"))
    }

    // ── Write operations ────────────────────────────────────────────

    /// Add a new job. Returns the job ID.
    pub async fn add_job(&self, job: CronJob) -> Result<String, String> {
        let id = {
            let mut store = self.state.store.lock().await;
            let id = service::ops::add_job(&mut store, job, self.state.clock.as_ref());
            store.persist()?;
            id
        };
        self.emit_change(&id, crate::gateway::events::ChangeKind::Created);
        Ok(id)
    }

    /// Update an existing job with partial changes.
    pub async fn update_job(
        &self,
        id: &str,
        updates: service::ops::CronJobUpdates,
    ) -> Result<(), String> {
        {
            let mut store = self.state.store.lock().await;
            service::ops::update_job(&mut store, id, updates, self.state.clock.as_ref())?;
            store.persist()?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Delete a job by ID.
    pub async fn delete_job(&self, id: &str) -> Result<(), String> {
        {
            let mut store = self.state.store.lock().await;
            service::ops::delete_job(&mut store, id)?;
            store.persist()?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Deleted);
        Ok(())
    }

    /// Enable a job by ID — and un-park it.
    ///
    /// Unconditional on purpose. `enabled` is not the whole story: phase 3
    /// parks a permanently-failed job by clearing `next_run_at_ms` while
    /// leaving `enabled == true`, and `recompute_next_run_maintenance` refuses
    /// to refill it while the permanent classification stands. The old
    /// `if !job.enabled` early return therefore short-circuited on exactly the
    /// jobs an operator reaches for this call to fix, reported success, and
    /// changed nothing — the job stayed dead. `enabled` was the wrong
    /// predicate; see [`CronJobView::parked`] for the right one.
    pub async fn enable_job(&self, id: &str) -> Result<(), String> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| format!("job not found: {id}"))?;
            job.enabled = true;
            job.updated_at = self.state.clock.now_ms();
            // Clear the permanent-failure classification *before* recomputing:
            // it is what parked the job, and leaving it behind would let the
            // next maintenance pass re-park it.
            job.state.last_error_reason = None;
            service::ops::recompute_next_run_full(job, self.state.clock.as_ref());
            store.persist()?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Disable a job by ID.
    pub async fn disable_job(&self, id: &str) -> Result<(), String> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| format!("job not found: {id}"))?;
            if job.enabled {
                job.enabled = false;
                job.state.next_run_at_ms = None;
                // Switching the job off withdraws any manual trigger that had
                // not been picked up yet; leaving it armed would mislabel
                // whatever *scheduled* run happens after the next enable.
                job.state.manual_trigger_pending = false;
                job.updated_at = self.state.clock.now_ms();
            }
            store.persist()?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Toggle a job's enabled state. Returns the new enabled state.
    pub async fn toggle_job(&self, id: &str) -> Result<bool, String> {
        let result = {
            let mut store = self.state.store.lock().await;
            let result = service::ops::toggle_job(&mut store, id, self.state.clock.as_ref())?;
            store.persist()?;
            result
        };
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(result)
    }

    /// Trigger a job to run immediately by setting its `next_run_at_ms` to now.
    /// The timer loop will pick it up on the next tick.
    ///
    /// The run is also stamped as manual (`JobStateV2::manual_trigger_pending`,
    /// consumed by `phase1_mark_due_jobs`). Pulling the schedule forward is
    /// otherwise byte-identical to the schedule coming due, and phase 3 acts on
    /// the difference: a `delete_after_run` one-shot must survive being run by
    /// hand before its appointment.
    pub async fn run_job(&self, id: &str) -> Result<(), String> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| format!("job not found: {id}"))?;
            if !job.enabled {
                return Err(format!("job '{id}' is disabled, enable it first"));
            }
            if job.state.running_at_ms.is_some() {
                return Err(format!("job '{id}' is already running"));
            }
            let now = self.state.clock.now_ms();
            job.state.next_run_at_ms = Some(now);
            job.state.manual_trigger_pending = true;
            store.persist()?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::StateChanged);
        Ok(())
    }

    /// Access the internal service state (for advanced use cases like timer loops).
    #[must_use]
    pub const fn state(&self) -> &Arc<ServiceState<SystemClock>> {
        &self.state
    }

    /// Build a scheduler-tick change emitter for the timer loop.
    ///
    /// Returns a closure that publishes `CronJobChanged` (StateChanged) for a
    /// job id, so the panel gets pushed the runtime-state updates a scheduled
    /// run writes in `phase3_writeback` — not just CRUD ops. Returns `None`
    /// when no event bus is wired (tests, CLI tooling).
    #[must_use]
    pub fn change_emitter(&self) -> Option<service::timer::ChangeEmitterFn> {
        let bus = self.event_bus.clone()?;
        Some(std::sync::Arc::new(move |job_id: &str| {
            use crate::gateway::events::{ChangeKind, GatewayEventFrame};
            let frame = GatewayEventFrame::CronJobChanged {
                job_id: job_id.to_string(),
                change: ChangeKind::StateChanged,
            };
            if let Err(e) = bus.publish_frame(&frame) {
                tracing::debug!(error = %e, "cron: failed to publish CronJobChanged frame (timer tick)");
            }
        }))
    }

    /// The most recent runs of one job, newest first.
    ///
    /// The store has held this history all along and the `cron.runs` RPC reads
    /// it, but nothing on the model-facing side did — so `cron_manage` could
    /// create, delete and manually trigger a job while being unable to answer
    /// "why does this keep failing?". That question is the one an operator
    /// actually asks, and R8 says the model should be able to answer it in
    /// conversation rather than sending someone to the Panel.
    pub async fn job_runs(
        &self,
        job_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::tasks::cron::history::CronRunRecord>, String> {
        let store = self.state.store.lock().await;
        store.get_runs(job_id, limit)
    }

    /// Request a graceful shutdown of the timer loop.
    ///
    /// `ServiceState::request_shutdown` and the loop's `is_shutdown()` check
    /// both existed, but nothing on `CronService` exposed the setter and the
    /// daemon's shutdown path only called heartbeat's — so the cron loop's exit
    /// arm was unreachable and it kept ticking (and dispatching jobs) while the
    /// rest of the process tore its dependencies down.
    pub fn request_shutdown(&self) {
        self.state.request_shutdown();
    }

    /// Wall-clock ms of the scheduler's last completed due-scan, or `0` when it
    /// has never scanned — the only honest answer to "is the scheduler alive?".
    ///
    /// See [`ServiceState::last_tick_at_ms`]: the timer loop's startup is
    /// conditional on an execution adapter being wired, while every `cron.*`
    /// handler is registered regardless.
    #[must_use]
    pub fn last_tick_at_ms(&self) -> i64 {
        self.state.last_tick_at_ms()
    }

    /// How often the timer loop wakes, after the same clamp the loop applies.
    /// Paired with [`Self::last_tick_at_ms`] so a caller can decide liveness
    /// without hard-coding the interval.
    #[must_use]
    pub fn check_interval_secs(&self) -> u64 {
        self.state.config.check_interval_secs.clamp(1, 60)
    }

    /// Delete cron run history older than `history_retention_days`
    /// (from this service's `CronConfig`). Used by the shared task reaper
    /// daemon ([[reaper]]) so the cron history table does not grow without
    /// bound. Returns the number of rows deleted.
    pub async fn reap_history(&self) -> Result<u64, String> {
        let now = self.state.clock.now_ms();
        let retention = self.state.config.history_retention_days;
        let store = self.state.store.lock().await;
        store.cleanup_old_runs(retention, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cron_service_basic_lifecycle() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.db").to_string_lossy().to_string();

        let config = CronConfig {
            db_path,
            ..CronConfig::default()
        };
        let service = CronService::new(config).unwrap();

        // List empty
        let jobs = service.list_jobs().await.unwrap();
        assert!(jobs.is_empty());

        // Add a job
        let job = CronJob::new(
            "Test Job",
            "agent-1",
            "do something",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        let id = service.add_job(job).await.unwrap();

        // Get it back
        let view = service.get_job(&id).await.unwrap();
        assert_eq!(view.name, "Test Job");
        assert!(view.state.next_run_at_ms.is_some());

        // List has one
        let jobs = service.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);

        // Toggle disable
        service.disable_job(&id).await.unwrap();
        let view = service.get_job(&id).await.unwrap();
        assert!(!view.enabled);
        assert!(view.state.next_run_at_ms.is_none());

        // Toggle enable
        service.enable_job(&id).await.unwrap();
        let view = service.get_job(&id).await.unwrap();
        assert!(view.enabled);
        assert!(view.state.next_run_at_ms.is_some());

        // Delete
        service.delete_job(&id).await.unwrap();
        let jobs = service.list_jobs().await.unwrap();
        assert!(jobs.is_empty());
    }

    /// A job parked by a permanent failure is `enabled == true` with nothing
    /// scheduled. `enable_job` used to short-circuit on `enabled` and report
    /// success while changing nothing, so the one call an operator reaches for
    /// to revive it was a no-op.
    #[tokio::test]
    async fn enable_job_unparks_a_permanently_failed_job() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.db").to_string_lossy().to_string();
        let service = CronService::new(CronConfig {
            db_path,
            ..CronConfig::default()
        })
        .unwrap();

        let id = service
            .add_job(CronJob::new(
                "Parked",
                "agent-1",
                "do something",
                ScheduleKind::Every {
                    every_ms: 60_000,
                    anchor_ms: None,
                },
            ))
            .await
            .unwrap();

        // Exactly the state phase 3 leaves behind on a permanent failure.
        {
            let mut store = service.state.store.lock().await;
            let job = store.get_job_mut(&id).unwrap();
            job.enabled = true;
            job.state.next_run_at_ms = None;
            job.state.last_run_status = Some(RunStatus::Error);
            job.state.last_error_reason = Some(ErrorReason::Permanent("agent missing".to_string()));
            store.persist().unwrap();
        }

        let parked = service.get_job(&id).await.unwrap();
        assert!(parked.enabled, "the switch is still on — that is the trap");
        assert!(parked.parked, "`parked` is what `enabled` cannot say");

        service.enable_job(&id).await.unwrap();

        let revived = service.get_job(&id).await.unwrap();
        assert!(
            revived.state.next_run_at_ms.is_some(),
            "enable must reschedule a parked job, not report success and leave it dead"
        );
        assert!(!revived.parked);
        assert!(
            revived.state.last_error_reason.is_none(),
            "the classification that parked it must not survive to re-park it"
        );
    }

    /// The store must follow `ALEPH_HOME` like every other subsystem.
    #[test]
    fn service_store_lands_under_aleph_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(dir.path());

        let service = CronService::new(CronConfig::default()).unwrap();
        drop(service);

        assert!(
            dir.path().join("data").join("tasks.db").exists(),
            "tasks.db must be created inside ALEPH_HOME"
        );
    }

    #[tokio::test]
    async fn cron_service_update() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.db").to_string_lossy().to_string();

        let config = CronConfig {
            db_path,
            ..CronConfig::default()
        };
        let service = CronService::new(config).unwrap();

        let job = CronJob::new(
            "Original",
            "agent-1",
            "original prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        let id = service.add_job(job).await.unwrap();

        let updates = service::ops::CronJobUpdates {
            name: Some("Updated".to_string()),
            prompt: Some("new prompt".to_string()),
            ..Default::default()
        };
        service.update_job(&id, updates).await.unwrap();

        let view = service.get_job(&id).await.unwrap();
        assert_eq!(view.name, "Updated");
        assert_eq!(view.prompt, "new prompt");
    }
}

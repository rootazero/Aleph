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
    JobChain, JobSnapshot, JobStateV2, RunStatus, ScheduleKind, SessionTarget, TriggerSource,
};

pub use crate::tasks::shared::error::TaskError;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;
use clock::{Clock, SystemClock};
use service::ServiceState;
use store::CronStore;

/// Shared handle to `CronService` for use in gateway handlers
pub type SharedCronService = Arc<tokio::sync::Mutex<CronService>>;

/// Process-global cron service, installed once at daemon boot.
///
/// The third of three: `goal::global()` and `looping::global()` already exist
/// for exactly this reason — `users.update`'s deactivation freeze is a free
/// function with no injected dependencies, and it has to reach every
/// subsystem that runs work on a principal's behalf. Cron was the one it could
/// not reach, and the exclusion was written down as a product fact ("`cron.*`
/// is admin-gated, so a deactivated MEMBER owns none by construction") that a
/// sibling handler in the same file falsifies: `users.create` accepts
/// `role: "admin"`, and a second admin is fully deactivatable.
///
/// `None` until boot, so tests and early-boot read as "no cron subsystem" and
/// the freeze skips the leg rather than failing.
///
/// `IndistinguishableDefault`, and this one came out OPPOSITE to the reflex
/// reading, so the derivation is written down rather than the verdict.
///
/// The reflex answer is `FailsOpen` — both readers are enforcement paths for a
/// walled principal, and a gate that stops gating is what that variant names.
/// It is wrong, because the enforcement does not stop. `executor.rs`'s
/// fire-time backstop reaches this handle only to make the disable DURABLE; the
/// `return make_error_result(..)` that refuses the run sits outside the
/// `if let Some(svc)`, so a walled owner's job is still refused at every fire
/// with the handle absent.
///
/// What absence actually produces is a value: `users.update`'s deactivation
/// freeze skips the cron leg and leaves `report.crons` at 0, which reads as
/// "this principal owned no cron jobs" and is indistinguishable from the truth.
/// The jobs stay enabled forever, retried and refused every tick, and the
/// operator is told the freeze covered everything.
static GLOBAL_CRON: CapabilitySlot<SharedCronService> = CapabilitySlot::new(
    "cron/service",
    MissingSemantics::IndistinguishableDefault {
        reads_as: "\"this principal owns no cron jobs\" -- the deactivation \
                   freeze skips the leg and reports crons: 0",
    },
);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_cron_slot() -> &'static dyn SlotStatus {
    &GLOBAL_CRON
}

/// Install the global service at boot. Idempotent: a second call is ignored.
pub fn init_global(service: SharedCronService) {
    let _ = GLOBAL_CRON.install(service);
}

/// Read the global service, if initialized.
#[must_use]
pub fn global() -> Option<SharedCronService> {
    GLOBAL_CRON.get().cloned()
}

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
    pub async fn list_jobs(&self) -> Result<Vec<CronJobView>, TaskError> {
        let store = self.state.store.lock().await;
        Ok(service::ops::list_jobs(&store))
    }

    /// Get a single job by ID as a read-only view.
    pub async fn get_job(&self, id: &str) -> Result<CronJobView, TaskError> {
        let store = self.state.store.lock().await;
        service::ops::get_job(&store, id).ok_or_else(|| TaskError::not_found("job", id))
    }

    /// Disable every enabled job owned by `user_id`, returning how many were
    /// disabled. The deactivation counterpart of `GoalStore::pause_all_owned_by`
    /// and `LoopRegistry::pause_all_owned_by`.
    ///
    /// Set-to-disabled rather than toggle: a sweep must be idempotent, and
    /// `toggle_job` on an already-disabled job would ENABLE it — arming
    /// background work during an offboarding.
    ///
    /// One-way, like its two siblings: reactivating the user does not
    /// re-enable these. The owner re-enables their own from a live session.
    pub async fn pause_all_owned_by(&self, user_id: &str) -> Result<usize, TaskError> {
        let paused: Vec<String> = {
            let mut store = self.state.store.lock().await;
            let mut ids = Vec::new();
            for job in store.jobs_mut() {
                if job.enabled && job.owner_user_id.as_deref() == Some(user_id) {
                    job.enabled = false;
                    ids.push(job.id.clone());
                }
            }
            if ids.is_empty() {
                return Ok(0);
            }
            store.persist().map_err(TaskError::internal)?;
            ids
        };
        for id in &paused {
            self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        }
        Ok(paused.len())
    }

    /// Fire-time liveness enforcement (round-5 ④, the qm `run-trigger.ts`
    /// criterion): the executor has just confirmed this job's owner is
    /// deactivated or gone, so the job is disabled BEFORE it would fire —
    /// the write-time sweep (`pause_all_owned_by`) is the primary, this is
    /// the backstop for anything it could miss (a job re-enabled by a second
    /// admin after the owner was walled, a freeze that raced the clock).
    ///
    /// Set-to-disabled, never toggle, for the same reason as the sweep:
    /// this path must be idempotent. Returns `false` when the job is already
    /// disabled (nothing changed, no frame emitted).
    pub async fn disable_walled_owner_job(&self, job_id: &str) -> Result<bool, TaskError> {
        let changed = {
            let mut store = self.state.store.lock().await;
            match store.get_job_mut(job_id) {
                Some(job) if job.enabled => {
                    job.enabled = false;
                    store.persist().map_err(TaskError::internal)?;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.emit_change(job_id, crate::gateway::events::ChangeKind::Updated);
        }
        Ok(changed)
    }

    // ── Write operations ────────────────────────────────────────────

    /// Add a new job. Returns the job ID.
    ///
    /// This is the write chokepoint for creation: `cron_manage` and the
    /// `cron.create` RPC both land here, so the chain check below is the one
    /// definition of a legal chain rather than one per surface. (The store-level
    /// `ops::add_job` stays unchecked on purpose — test fixtures build cyclic
    /// graphs deliberately to exercise the fire-time guards.)
    ///
    /// Every rejection `chain::validate` can produce — empty target, self-link,
    /// missing target, cycle — is a caller error, so the classification is made
    /// once here rather than threaded through `chain.rs`. Reported as
    /// [`TaskError::Invalid`] and not `NotFound` even for a missing target: the
    /// job the caller *addressed* is the one being written, and answering
    /// "not found" to a create would name the wrong thing. What is missing is
    /// the value of the `chain` parameter.
    pub async fn add_job(&self, job: CronJob) -> Result<String, TaskError> {
        let id = {
            let mut store = self.state.store.lock().await;
            chain::validate(&store, &job.id, job.chain().as_ref()).map_err(TaskError::invalid)?;
            let id = service::ops::add_job(&mut store, job, self.state.clock.as_ref());
            store.persist().map_err(TaskError::internal)?;
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
    ) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            // Same predicate as `add_job`, held under the same lock as the
            // write it guards — validating outside the lock would let a
            // concurrent delete turn a checked target into a missing one.
            if let Some(chain) = updates.chain.as_ref() {
                chain::validate(&store, id, chain.as_ref()).map_err(TaskError::invalid)?;
            }
            service::ops::update_job(&mut store, id, updates, self.state.clock.as_ref())?;
            store.persist().map_err(TaskError::internal)?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Delete a job by ID.
    pub async fn delete_job(&self, id: &str) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            service::ops::delete_job(&mut store, id)?;
            store.persist().map_err(TaskError::internal)?;
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
    pub async fn enable_job(&self, id: &str) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| TaskError::not_found("job", id))?;
            job.enabled = true;
            job.updated_at = self.state.clock.now_ms();
            // Clear the permanent-failure classification *before* recomputing:
            // it is what parked the job, and leaving it behind would let the
            // next maintenance pass re-park it.
            job.state.last_error_reason = None;
            service::ops::recompute_next_run_full(job, self.state.clock.as_ref());
            store.persist().map_err(TaskError::internal)?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Disable a job by ID.
    pub async fn disable_job(&self, id: &str) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| TaskError::not_found("job", id))?;
            if job.enabled {
                job.enabled = false;
                job.state.next_run_at_ms = None;
                // Switching the job off withdraws any manual trigger that had
                // not been picked up yet; leaving it armed would mislabel
                // whatever *scheduled* run happens after the next enable.
                job.state.manual_trigger_pending = false;
                job.updated_at = self.state.clock.now_ms();
            }
            store.persist().map_err(TaskError::internal)?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Toggle a job's enabled state. Returns the new enabled state.
    pub async fn toggle_job(&self, id: &str) -> Result<bool, TaskError> {
        let result = {
            let mut store = self.state.store.lock().await;
            let result = service::ops::toggle_job(&mut store, id, self.state.clock.as_ref())?;
            store.persist().map_err(TaskError::internal)?;
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
    ///
    /// The two state refusals below are [`TaskError::Invalid`], not a variant
    /// of their own: the request named a real job and was well-formed, the job
    /// is simply in a state that refuses it. See
    /// [`TaskError`][crate::tasks::shared::error::TaskError] for why there is
    /// no `Conflict`.
    pub async fn run_job(&self, id: &str) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            let job = store
                .get_job_mut(id)
                .ok_or_else(|| TaskError::not_found("job", id))?;
            if !job.enabled {
                return Err(TaskError::invalid(format!(
                    "job '{id}' is disabled, enable it first"
                )));
            }
            if job.state.running_at_ms.is_some() {
                return Err(TaskError::invalid(format!("job '{id}' is already running")));
            }
            let now = self.state.clock.now_ms();
            job.state.next_run_at_ms = Some(now);
            job.state.manual_trigger_pending = true;
            store.persist().map_err(TaskError::internal)?;
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
    ) -> Result<Vec<crate::tasks::cron::history::CronRunRecord>, TaskError> {
        let store = self.state.store.lock().await;
        store.get_runs(job_id, limit).map_err(TaskError::internal)
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
    pub async fn reap_history(&self) -> Result<u64, TaskError> {
        let now = self.state.clock.now_ms();
        let retention = self.state.config.history_retention_days;
        let store = self.state.store.lock().await;
        store
            .cleanup_old_runs(retention, now)
            .map_err(TaskError::internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deactivation sweep's third leg. Two properties, and the second is
    /// the reason this is `enabled = false` rather than `toggle_job`:
    ///
    /// - it is scoped to the principal, so offboarding one person does not
    ///   silence everyone else's schedules;
    /// - it is idempotent, so a second sweep (a repeated
    ///   `users.update{status:"deactivated"}`, or a retry) cannot ENABLE an
    ///   already-disabled job — which is what a toggle would do, arming
    ///   background work in the middle of an offboarding.
    #[tokio::test]
    async fn pausing_by_owner_is_scoped_and_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let service = CronService::new(CronConfig {
            db_path: dir.path().join("cron.db").to_string_lossy().to_string(),
            ..CronConfig::default()
        })
        .unwrap();

        let make = |name: &str, owner: Option<&str>, enabled: bool| {
            let mut job = CronJob::new(
                name,
                "agent-1",
                "do something",
                ScheduleKind::Every {
                    every_ms: 60_000,
                    anchor_ms: None,
                },
            );
            job.owner_user_id = owner.map(str::to_string);
            job.enabled = enabled;
            job
        };
        let alice_on = make("alice-live", Some("u-alice"), true);
        let alice_off = make("alice-already-off", Some("u-alice"), false);
        let bob_on = make("bob-live", Some("u-bob"), true);
        let unowned = make("legacy", None, true);
        for job in [
            alice_on.clone(),
            alice_off.clone(),
            bob_on.clone(),
            unowned.clone(),
        ] {
            service.add_job(job).await.unwrap();
        }

        assert_eq!(
            service.pause_all_owned_by("u-alice").await.unwrap(),
            1,
            "only the ENABLED job of that principal counts as newly paused"
        );

        let by_name = |jobs: &[CronJobView], name: &str| {
            jobs.iter()
                .find(|j| j.name == name)
                .expect("job present")
                .enabled
        };
        let jobs = service.list_jobs().await.unwrap();
        assert!(!by_name(&jobs, "alice-live"));
        assert!(!by_name(&jobs, "alice-already-off"));
        assert!(
            by_name(&jobs, "bob-live"),
            "another principal's schedule must survive"
        );
        assert!(
            by_name(&jobs, "legacy"),
            "a pre-P1 job with no owner belongs to nobody in particular and \
             must not be swept by a per-principal offboarding"
        );

        assert_eq!(
            service.pause_all_owned_by("u-alice").await.unwrap(),
            0,
            "a second sweep is a no-op, not a re-toggle"
        );
        let jobs = service.list_jobs().await.unwrap();
        assert!(!by_name(&jobs, "alice-live"));
        assert!(!by_name(&jobs, "alice-already-off"));
    }

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

    // ── Job chaining ────────────────────────────────────────────────
    //
    // The trigger machinery, its cycle detection and its tests shipped whole;
    // what was missing until 2026-08-09 was any writer of the two link fields
    // outside `#[cfg(test)]`. These cover the write half.

    fn test_service(dir: &tempfile::TempDir) -> CronService {
        CronService::new(CronConfig {
            db_path: dir.path().join("cron.db").to_string_lossy().to_string(),
            ..CronConfig::default()
        })
        .unwrap()
    }

    fn named_job(name: &str) -> CronJob {
        CronJob::new(
            name,
            "agent-1",
            "prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        )
    }

    #[tokio::test]
    async fn a_chain_set_at_create_is_stored_and_readable() {
        let dir = tempfile::TempDir::new().unwrap();
        let service = test_service(&dir);

        let target = service.add_job(named_job("target")).await.unwrap();
        let recovery = service.add_job(named_job("recovery")).await.unwrap();

        let mut job = named_job("source");
        job.set_chain(Some(JobChain {
            on_success: Some(target.clone()),
            on_failure: Some(recovery.clone()),
        }));
        let id = service.add_job(job).await.unwrap();

        // Read-back through the view is the half that makes this a wire and
        // not a write-only field.
        let view = service.get_job(&id).await.unwrap();
        let chain = view.chain.expect("chain must survive the round trip");
        assert_eq!(chain.on_success.as_deref(), Some(target.as_str()));
        assert_eq!(chain.on_failure.as_deref(), Some(recovery.as_str()));

        // And it must be the field `phase3` actually reads, not a parallel one.
        let stored = service.state.store.lock().await;
        let raw = stored.get_job(&id).unwrap();
        assert_eq!(raw.next_job_id_on_success.as_deref(), Some(target.as_str()));
        assert_eq!(
            raw.next_job_id_on_failure.as_deref(),
            Some(recovery.as_str())
        );
    }

    #[tokio::test]
    async fn update_replaces_the_whole_chain_and_can_clear_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let service = test_service(&dir);

        let a = service.add_job(named_job("a")).await.unwrap();
        let b = service.add_job(named_job("b")).await.unwrap();

        let mut job = named_job("source");
        job.set_chain(Some(JobChain {
            on_success: Some(a.clone()),
            on_failure: Some(b.clone()),
        }));
        let id = service.add_job(job).await.unwrap();

        // Replace, not merge: sending only `on_success` drops `on_failure`.
        service
            .update_job(
                &id,
                service::ops::CronJobUpdates {
                    chain: Some(Some(JobChain {
                        on_success: Some(b.clone()),
                        on_failure: None,
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let chain = service.get_job(&id).await.unwrap().chain.unwrap();
        assert_eq!(chain.on_success.as_deref(), Some(b.as_str()));
        assert!(chain.on_failure.is_none(), "replace, not merge");

        // Outer `None` leaves it alone — an unrelated update must not erase it.
        service
            .update_job(
                &id,
                service::ops::CronJobUpdates {
                    name: Some("renamed".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(service.get_job(&id).await.unwrap().chain.is_some());

        // Outer `Some(None)` clears.
        service
            .update_job(
                &id,
                service::ops::CronJobUpdates {
                    chain: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(service.get_job(&id).await.unwrap().chain.is_none());
    }

    /// A chain to a job that does not exist never fires and says nothing when
    /// it doesn't. Rejecting at write time is the only moment the caller can
    /// still act on it.
    #[tokio::test]
    async fn a_chain_to_a_nonexistent_job_is_refused_at_write_time() {
        let dir = tempfile::TempDir::new().unwrap();
        let service = test_service(&dir);

        let mut job = named_job("source");
        job.set_chain(Some(JobChain {
            on_success: Some("no-such-job".to_string()),
            on_failure: None,
        }));
        let err = service.add_job(job).await.unwrap_err();
        // `Invalid`, not `NotFound`: the job the caller addressed is the one
        // being created. What is missing is the value of the `chain` field, so
        // the RPC face answers INVALID_PARAMS rather than a 404 naming the
        // wrong object. See `gateway::handlers::task_error`.
        assert!(matches!(err, TaskError::Invalid(_)), "{err:?}");
        assert!(
            err.message().contains("no-such-job"),
            "the refusal must name the target: {err}"
        );

        // ...and nothing was stored.
        assert!(service.list_jobs().await.unwrap().is_empty());
    }

    /// Cycles and self-links are refused on update.
    ///
    /// Update, not create: both create faces mint the job id with
    /// `CronJob::new`, so a brand-new job cannot yet be anyone's successor and
    /// cannot close a loop. Create still runs the same predicate — one
    /// definition of a legal chain for both faces beats two that agree today.
    #[tokio::test]
    async fn a_cycle_and_a_self_link_are_refused_on_update() {
        let dir = tempfile::TempDir::new().unwrap();
        let service = test_service(&dir);

        let a = service.add_job(named_job("a")).await.unwrap();
        let mut b_job = named_job("b");
        b_job.set_chain(Some(JobChain {
            on_success: Some(a.clone()),
            on_failure: None,
        }));
        let b = service.add_job(b_job).await.unwrap();

        // a -> b would close the loop b -> a.
        let err = service
            .update_job(
                &a,
                service::ops::CronJobUpdates {
                    chain: Some(Some(JobChain {
                        on_success: Some(b.clone()),
                        on_failure: None,
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TaskError::Invalid(_)), "{err:?}");
        assert!(err.message().contains("cycle"), "{err}");

        // The failure leg is walked too — a loop through `on_failure` is the
        // same loop.
        let err = service
            .update_job(
                &a,
                service::ops::CronJobUpdates {
                    chain: Some(Some(JobChain {
                        on_success: None,
                        on_failure: Some(b.clone()),
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.message().contains("cycle"), "{err}");

        // A self-link is the degenerate cycle and gets its own message.
        let err = service
            .update_job(
                &a,
                service::ops::CronJobUpdates {
                    chain: Some(Some(JobChain {
                        on_success: Some(a.clone()),
                        on_failure: None,
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.message().contains("itself"), "{err}");

        // None of the refusals wrote anything.
        assert!(service.get_job(&a).await.unwrap().chain.is_none());
    }

    /// Both write faces must reach the links through `set_chain`, never by
    /// assigning `next_job_id_on_*` directly.
    ///
    /// A direct assignment compiles, passes review and skips the existence /
    /// cycle check entirely — the two surfaces would then disagree about what
    /// a legal chain is, which is how "the tool accepted it but the RPC didn't"
    /// bugs are born. Source-level because at runtime a bypassed check and an
    /// absent one are indistinguishable.
    #[test]
    fn no_write_surface_assigns_the_chain_links_directly() {
        const SURFACES: [(&str, &str); 2] = [
            (
                "src/builtin_tools/cron_manage.rs",
                include_str!("../../builtin_tools/cron_manage.rs"),
            ),
            (
                "src/gateway/handlers/cron/real.rs",
                include_str!("../../gateway/handlers/cron/real.rs"),
            ),
        ];

        let mut reaches_the_setter = 0usize;
        for (path, src) in SURFACES {
            let production = crate::utils::source_scan::production_prefix(src);
            for field in ["next_job_id_on_success", "next_job_id_on_failure"] {
                assert!(
                    !production.contains(&format!("{field} =")),
                    "{path} assigns `{field}` directly — go through \
                     `CronJob::set_chain` so `chain::validate` still runs"
                );
            }
            if production.contains("set_chain(") {
                reaches_the_setter += 1;
            }
        }
        assert_eq!(
            reaches_the_setter,
            SURFACES.len(),
            "both write surfaces must call `set_chain` — a surface that stopped \
             doing so has either dropped the feature or grown its own path to it"
        );
    }

    /// The `reads_as` sentence reaches an operator, and this one had to be
    /// re-derived: the reflex verdict for a handle whose only readers are
    /// enforcement paths is `FailsOpen`, and it is wrong here because the
    /// fire-time refusal does not depend on this handle. See the static's doc.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_cron_slot();
        assert_eq!(slot.id(), "cron/service");
        let MissingSemantics::IndistinguishableDefault { reads_as } = slot.missing() else {
            panic!(
                "expected IndistinguishableDefault, got {:?}",
                slot.missing()
            );
        };
        assert!(
            reads_as.contains("crons: 0"),
            "must name the value users.update's freeze really reports; got {reads_as:?}"
        );
    }
}

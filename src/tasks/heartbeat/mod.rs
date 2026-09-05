//! Heartbeat monitoring subsystem.
//!
//! Provides periodic L1 probe checks with L2 agent analysis when triggered.

pub mod config;
pub mod dedup;
pub mod executor;
pub mod history;
pub mod probe;
pub mod service;
pub mod store;
pub mod wake;

use tokio::sync::Mutex;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

use crate::tasks::heartbeat::config::{HeartbeatConfig, HeartbeatTask, HeartbeatTaskView};
use crate::tasks::heartbeat::service::ops;
use crate::tasks::heartbeat::service::HeartbeatServiceState;
use crate::tasks::heartbeat::store::HeartbeatStore;
use crate::tasks::shared::clock::Clock;
pub use crate::tasks::shared::error::TaskError;

/// Shared handle to the `HeartbeatService`.
pub type SharedHeartbeatService = Arc<Mutex<HeartbeatService>>;

/// Process-global heartbeat service, installed once at daemon boot.
///
/// The fourth of four. `goal::global()`, `looping::global()` and
/// `tasks::cron::global()` already existed for one reason —
/// `users.update`'s deactivation freeze is a free function with no injected
/// dependencies, and it has to reach every subsystem that runs work on a
/// principal's behalf. Heartbeat was the one it could not reach, and unlike
/// cron the exclusion was never even argued: the service is injected as an
/// `Option<SharedHeartbeatService>` at `start/mod.rs`, so the freeze had no
/// name to call. `heartbeat.*` is gated on exactly cron's terms, and
/// `users.create` accepts `role: "admin"` — a second admin is fully
/// deactivatable and their heartbeats kept firing, and kept delivering
/// through `delivery_config`, after the receipt said the freeze was done.
///
/// `ConsumerDecides`, NOT the cron twin's `IndistinguishableDefault`, and the
/// derivation is written down because copying the twin is the reflex.
/// `GLOBAL_CRON` earns that variant honestly:
/// `aleph_protocol::users::FrozenBackgroundWork::crons` is a `usize` that
/// stays `0` when the handle is absent, so the receipt says "this principal
/// owned no cron jobs" and no reader can tell. The heartbeat leg is an
/// `Option<usize>` on that same struct — which the freeze, the `users.get`
/// preview and the wire all now share, rather than a private twin of it —
/// precisely so
/// absence CANNOT read as a measured zero — the one consumer, this module's
/// caller in `gateway::handlers::users::freeze_owned_heartbeats`, decides
/// that `None` means "not measured" and both the receipt and the CLI say so
/// in their own sentence. Absence is still a real defect (the principal's
/// tasks stay armed), which is why the handle is on the roster at all; it is
/// simply no longer an INDISTINGUISHABLE one.
///
/// `None` until boot, so tests and early boot read as "no heartbeat
/// subsystem" and the freeze reports the leg unmeasured rather than failing.
static GLOBAL_HEARTBEAT: CapabilitySlot<SharedHeartbeatService> =
    CapabilitySlot::new("heartbeat/service", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_heartbeat_slot() -> &'static dyn SlotStatus {
    &GLOBAL_HEARTBEAT
}

/// Install the global service at boot. Idempotent: a second call is ignored.
pub fn init_global(service: SharedHeartbeatService) {
    let _ = GLOBAL_HEARTBEAT.install(service);
}

/// Record that boot reached this slot and had nothing to install.
///
/// Boot has three such arms — `[heartbeat] enabled = false`, the data
/// directory failing to resolve, and `HeartbeatStore::open` failing — and
/// they need different sentences: the first is a setting an operator chose,
/// the other two are faults they have to fix. `because` is quoted verbatim
/// to an operator by the `core/capability-wiring` diagnostic.
pub fn decline_global(because: &'static str) {
    GLOBAL_HEARTBEAT.decline(because);
}

/// Read the global service, if initialized.
#[must_use]
pub fn global() -> Option<SharedHeartbeatService> {
    GLOBAL_HEARTBEAT.get().cloned()
}

/// Top-level heartbeat service facade.
///
/// Wraps the service state and wake queue, providing CRUD operations
/// and access to internals for the timer loop.
pub struct HeartbeatService {
    state: Arc<HeartbeatServiceState>,
    wake_queue: Arc<wake::WakeQueue>,
    /// Optional event-bus handle, mirrored from [[`CronService`]]. Wired via
    /// [`with_event_bus`] at startup so each successful mutation publishes
    /// `HeartbeatTaskChanged` for the panel to drop polling.
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl HeartbeatService {
    /// Create a new `HeartbeatService` with the given store and config.
    pub fn new(store: HeartbeatStore, config: HeartbeatConfig) -> Self {
        let state = Arc::new(HeartbeatServiceState::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            config,
        ));
        Self {
            state,
            wake_queue: Arc::new(wake::WakeQueue::new()),
            event_bus: None,
        }
    }

    /// Builder: attach an event bus for `HeartbeatTaskChanged` push.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    fn emit_change(&self, task_id: &str, change: crate::gateway::events::ChangeKind) {
        if let Some(bus) = &self.event_bus {
            let frame = crate::gateway::events::GatewayEventFrame::HeartbeatTaskChanged {
                task_id: task_id.to_string(),
                change,
            };
            if let Err(e) = bus.publish_frame(&frame) {
                tracing::debug!(error = %e, "heartbeat: failed to publish HeartbeatTaskChanged frame");
            }
        }
    }

    /// Access the shared service state (for the timer loop).
    #[must_use]
    pub const fn state(&self) -> &Arc<HeartbeatServiceState> {
        &self.state
    }

    /// Access the wake queue (for external wake triggers).
    #[must_use]
    pub const fn wake_queue(&self) -> &Arc<wake::WakeQueue> {
        &self.wake_queue
    }

    /// Build a timer-tick change emitter for the heartbeat loop.
    ///
    /// Returns a closure that publishes `HeartbeatTaskChanged` (StateChanged)
    /// for a task id, so the panel gets pushed the runtime-state updates a
    /// scheduled run writes in `writeback_one` — not just CRUD ops. Returns
    /// `None` when no event bus is wired (tests, CLI tooling).
    #[must_use]
    pub fn change_emitter(&self) -> Option<service::timer::ChangeEmitterFn> {
        let bus = self.event_bus.clone()?;
        Some(std::sync::Arc::new(move |task_id: &str| {
            use crate::gateway::events::{ChangeKind, GatewayEventFrame};
            let frame = GatewayEventFrame::HeartbeatTaskChanged {
                task_id: task_id.to_string(),
                change: ChangeKind::StateChanged,
            };
            if let Err(e) = bus.publish_frame(&frame) {
                tracing::debug!(error = %e, "heartbeat: failed to publish HeartbeatTaskChanged frame (timer tick)");
            }
        }))
    }

    /// List all tasks as read-only views.
    pub async fn list_tasks(&self) -> Vec<HeartbeatTaskView> {
        let store = self.state.store.lock().await;
        ops::list_tasks(&store)
    }

    /// Get a single task as a read-only view.
    pub async fn get_task(&self, id: &str) -> Option<HeartbeatTaskView> {
        let store = self.state.store.lock().await;
        ops::get_task(&store, id)
    }

    /// Add a new task. Returns the task ID.
    ///
    /// Rejects duplicate ids at the service boundary so the caller sees a
    /// proper "already exists" error rather than a generic storage failure
    /// from the SQLite `UNIQUE` constraint. Mirrors `CronService::add_job`.
    pub async fn add_task<C: Clock>(
        &self,
        task: HeartbeatTask,
        clock: &C,
    ) -> Result<String, TaskError> {
        let id = {
            let mut store = self.state.store.lock().await;
            if store.get_task(&task.id).is_some() {
                return Err(TaskError::invalid(format!(
                    "task with id '{}' already exists",
                    task.id
                )));
            }
            let id = ops::add_task(&mut store, task, clock);
            store.persist().map_err(TaskError::internal)?;
            id
        };
        self.emit_change(&id, crate::gateway::events::ChangeKind::Created);
        Ok(id)
    }

    /// Apply partial updates to an existing task.
    pub async fn update_task<C: Clock>(
        &self,
        id: &str,
        updates: ops::HeartbeatTaskUpdates,
        clock: &C,
    ) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            ops::update_task(&mut store, id, updates, clock)?;
            store.persist().map_err(TaskError::internal)?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(())
    }

    /// Delete a task by ID.
    pub async fn delete_task(&self, id: &str) -> Result<(), TaskError> {
        {
            let mut store = self.state.store.lock().await;
            ops::delete_task(&mut store, id)?;
            store.persist().map_err(TaskError::internal)?;
        }
        self.emit_change(id, crate::gateway::events::ChangeKind::Deleted);
        Ok(())
    }

    /// Toggle a task's enabled state. Returns the new enabled state.
    pub async fn toggle_task<C: Clock>(&self, id: &str, clock: &C) -> Result<bool, TaskError> {
        let enabled = {
            let mut store = self.state.store.lock().await;
            let enabled = ops::toggle_task(&mut store, id, clock)?;
            store.persist().map_err(TaskError::internal)?;
            enabled
        };
        self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        Ok(enabled)
    }

    /// Disable every enabled task owned by `user_id`, returning how many were
    /// disabled. The heartbeat leg of `users.update`'s deactivation freeze —
    /// the deactivation counterpart of `CronService::pause_all_owned_by`,
    /// `GoalStore::pause_all_owned_by` and `LoopRegistry::pause_all_owned_by`.
    ///
    /// One-way, like its three siblings: reactivating the user does not
    /// re-enable these. The owner re-enables their own from a live session
    /// (`heartbeat_toggle`), and the reactivation receipt says so.
    ///
    /// The sweep itself — including which rows it refuses to touch — lives in
    /// [`ops::pause_all_owned_by`]. This wrapper owns only persistence and
    /// the change frames, so a no-op sweep writes nothing and emits nothing.
    pub async fn pause_all_owned_by(&self, user_id: &str) -> Result<usize, TaskError> {
        let paused = {
            let mut store = self.state.store.lock().await;
            let paused = ops::pause_all_owned_by(&mut store, user_id);
            if paused.is_empty() {
                return Ok(0);
            }
            store.persist().map_err(TaskError::internal)?;
            paused
        };
        for id in &paused {
            self.emit_change(id, crate::gateway::events::ChangeKind::Updated);
        }
        Ok(paused.len())
    }

    /// How many tasks `user_id` OWNS — the read-only counterpart of
    /// [`Self::pause_all_owned_by`] and the heartbeat leg of `users.get`'s
    /// dossier. The selection (including its deliberate difference from the
    /// sweep's) lives in [`ops::count_owned_by`]; this wrapper only takes the
    /// lock, and never persists — a count changes nothing.
    pub async fn count_owned_by(&self, user_id: &str) -> usize {
        let store = self.state.store.lock().await;
        ops::count_owned_by(&store, user_id)
    }

    /// Request a graceful shutdown of the timer loop.
    pub fn request_shutdown(&self) {
        self.state.request_shutdown();
    }

    /// Delete heartbeat run history older than `history_retention_days`
    /// (from this service's `HeartbeatConfig`). Called periodically by the
    /// shared task reaper daemon ([[reaper]]). Returns rows deleted.
    pub async fn reap_history(&self) -> Result<usize, TaskError> {
        let retention = self.state.config.history_retention_days;
        let store = self.state.store.lock().await;
        store
            .cleanup_old_runs(retention)
            .map_err(TaskError::internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};
    use crate::tasks::shared::clock::testing::FakeClock;

    fn make_service() -> HeartbeatService {
        let store = HeartbeatStore::open_in_memory().unwrap();
        HeartbeatService::new(store, HeartbeatConfig::default())
    }

    fn make_task(name: &str) -> HeartbeatTask {
        HeartbeatTask::new(
            name.to_string(),
            "main".to_string(),
            60_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::Always,
            },
        )
    }

    #[tokio::test]
    async fn service_crud_lifecycle() {
        let service = make_service();
        let clock = FakeClock::new(1_000_000);

        // Add
        let task = make_task("Test Task");
        let id = service.add_task(task, &clock).await.unwrap();
        assert_eq!(service.list_tasks().await.len(), 1);

        // Get
        let view = service.get_task(&id).await.unwrap();
        assert_eq!(view.name, "Test Task");

        // Update
        let updates = ops::HeartbeatTaskUpdates {
            name: Some("Updated".to_string()),
            ..Default::default()
        };
        service.update_task(&id, updates, &clock).await.unwrap();
        let view = service.get_task(&id).await.unwrap();
        assert_eq!(view.name, "Updated");

        // Toggle
        let enabled = service.toggle_task(&id, &clock).await.unwrap();
        assert!(!enabled);

        // Delete
        service.delete_task(&id).await.unwrap();
        assert!(service.list_tasks().await.is_empty());
    }

    #[tokio::test]
    async fn service_shutdown_flag() {
        let service = make_service();
        assert!(!service.state().is_shutdown());
        service.request_shutdown();
        assert!(service.state().is_shutdown());
    }

    /// Duplicate id at the service boundary surfaces as `Invalid`, not
    /// `Internal`: before this guard the SQLite `UNIQUE` constraint
    /// propagated as a generic storage failure indistinguishable from a real
    /// write bug. Mirrors the cron service-level guard.
    #[tokio::test]
    async fn add_task_with_duplicate_id_is_refused_as_invalid() {
        let service = make_service();
        let clock = FakeClock::new(1_000_000);

        let first = service.add_task(make_task("first"), &clock).await.unwrap();

        // Same id, different name → service-level duplicate detection.
        let mut dup = make_task("second");
        dup.id = first.clone();
        let err = service.add_task(dup, &clock).await.unwrap_err();
        assert!(
            matches!(err, TaskError::Invalid(_)),
            "duplicate id must be Invalid, got {err:?}"
        );
        assert!(
            err.message().contains(&first),
            "the refusal must name the conflicting id: {err}"
        );
        assert!(
            err.message().contains("already exists"),
            "the refusal must explain the cause: {err}"
        );

        // The original task must still be there, untouched.
        let tasks = service.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, first);
        assert_eq!(tasks[0].name, "first");
    }

    /// The `MissingSemantics` variant is the operator-facing severity of this
    /// handle going missing, and this one is deliberately NOT the cron twin's
    /// `IndistinguishableDefault` — see the static's doc for the derivation.
    /// Reclassifying it must land somewhere red rather than quietly changing
    /// what `aleph doctor` says.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_heartbeat_slot();
        assert_eq!(slot.id(), "heartbeat/service");
        assert_eq!(
            slot.missing(),
            MissingSemantics::ConsumerDecides,
            "absence of this handle is DISTINGUISHABLE — the freeze's heartbeat \
             leg is an Option and reports 'not measured' — so the twin's \
             IndistinguishableDefault would overstate what a reader cannot tell"
        );
    }

    /// The freeze sweep through the service: persistence and the count, on top
    /// of `ops::pause_all_owned_by`'s row selection.
    #[tokio::test]
    async fn the_service_freeze_disables_the_owners_tasks_and_counts_them() {
        let service = make_service();
        let clock = FakeClock::new(1_000_000);

        let mut mine = make_task("mine");
        mine.id = "hb-mine".to_string();
        mine.owner_user_id = Some("u-alice".to_string());
        service.add_task(mine, &clock).await.unwrap();

        let mut theirs = make_task("theirs");
        theirs.id = "hb-theirs".to_string();
        theirs.owner_user_id = Some("u-bob".to_string());
        service.add_task(theirs, &clock).await.unwrap();

        assert_eq!(service.pause_all_owned_by("u-alice").await.unwrap(), 1);
        assert_eq!(
            service.get_task("hb-mine").await.map(|t| t.enabled),
            Some(false),
            "the count is not the claim — the stored row is"
        );
        assert_eq!(
            service.get_task("hb-theirs").await.map(|t| t.enabled),
            Some(true)
        );
        assert_eq!(
            service.pause_all_owned_by("u-alice").await.unwrap(),
            0,
            "idempotent: the second sweep changed nothing and says so"
        );
    }
}

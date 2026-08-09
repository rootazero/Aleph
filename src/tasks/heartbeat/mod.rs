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

use crate::sync_primitives::Arc;

use crate::tasks::heartbeat::config::{HeartbeatConfig, HeartbeatTask, HeartbeatTaskView};
use crate::tasks::heartbeat::service::ops;
use crate::tasks::heartbeat::service::HeartbeatServiceState;
use crate::tasks::heartbeat::store::HeartbeatStore;
use crate::tasks::shared::clock::Clock;
pub use crate::tasks::shared::error::TaskError;

/// Shared handle to the `HeartbeatService`.
pub type SharedHeartbeatService = Arc<Mutex<HeartbeatService>>;

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
    pub async fn add_task<C: Clock>(
        &self,
        task: HeartbeatTask,
        clock: &C,
    ) -> Result<String, TaskError> {
        let id = {
            let mut store = self.state.store.lock().await;
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
}

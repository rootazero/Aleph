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

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::tasks::heartbeat::config::{HeartbeatConfig, HeartbeatTask, HeartbeatTaskView};
use crate::tasks::heartbeat::service::ops;
use crate::tasks::heartbeat::service::HeartbeatServiceState;
use crate::tasks::heartbeat::store::HeartbeatStore;
use crate::tasks::shared::clock::Clock;

/// Shared handle to the HeartbeatService.
pub type SharedHeartbeatService = Arc<Mutex<HeartbeatService>>;

/// Top-level heartbeat service facade.
///
/// Wraps the service state and wake queue, providing CRUD operations
/// and access to internals for the timer loop.
pub struct HeartbeatService {
    state: Arc<HeartbeatServiceState>,
    wake_queue: Arc<wake::WakeQueue>,
}

impl HeartbeatService {
    /// Create a new HeartbeatService with the given store and config.
    pub fn new(store: HeartbeatStore, config: HeartbeatConfig) -> Self {
        let state = Arc::new(HeartbeatServiceState::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            config,
        ));
        Self {
            state,
            wake_queue: Arc::new(wake::WakeQueue::new()),
        }
    }

    /// Access the shared service state (for the timer loop).
    pub fn state(&self) -> &Arc<HeartbeatServiceState> {
        &self.state
    }

    /// Access the wake queue (for external wake triggers).
    pub fn wake_queue(&self) -> &Arc<wake::WakeQueue> {
        &self.wake_queue
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
    pub async fn add_task<C: Clock>(&self, task: HeartbeatTask, clock: &C) -> String {
        let mut store = self.state.store.lock().await;
        let id = ops::add_task(&mut store, task, clock);
        let _ = store.persist();
        id
    }

    /// Apply partial updates to an existing task.
    pub async fn update_task<C: Clock>(
        &self,
        id: &str,
        updates: ops::HeartbeatTaskUpdates,
        clock: &C,
    ) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        ops::update_task(&mut store, id, updates, clock)?;
        let _ = store.persist();
        Ok(())
    }

    /// Delete a task by ID.
    pub async fn delete_task(&self, id: &str) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        ops::delete_task(&mut store, id)?;
        let _ = store.persist();
        Ok(())
    }

    /// Toggle a task's enabled state. Returns the new enabled state.
    pub async fn toggle_task<C: Clock>(&self, id: &str, clock: &C) -> Result<bool, String> {
        let mut store = self.state.store.lock().await;
        let enabled = ops::toggle_task(&mut store, id, clock)?;
        let _ = store.persist();
        Ok(enabled)
    }

    /// Request a graceful shutdown of the timer loop.
    pub fn request_shutdown(&self) {
        self.state.request_shutdown();
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
        let id = service.add_task(task, &clock).await;
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

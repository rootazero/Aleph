use std::pin::Pin;

use futures::Stream;

use crate::a2a::domain::{TaskArtifactUpdateEvent, TaskStatusUpdateEvent, UpdateEvent};

use super::task_manager::A2AResult;

/// Port for real-time streaming of task updates.
///
/// Supports pub/sub semantics: subscribers receive status and artifact events
/// as they occur, and broadcasters push events to all active subscribers.
#[async_trait::async_trait]
pub trait A2AStreamingHandler: Send + Sync {
    /// Subscribe to all update events (status + artifact) for a task
    async fn subscribe_all(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>>;

    /// Broadcast a status update to all subscribers of a task
    async fn broadcast_status(&self, task_id: &str, update: TaskStatusUpdateEvent)
        -> A2AResult<()>;

    /// Broadcast an artifact update to all subscribers of a task.
    ///
    /// Default implementation is a no-op — a handler that carries status
    /// events only does not have to answer this one.
    async fn broadcast_artifact(
        &self,
        _task_id: &str,
        _event: TaskArtifactUpdateEvent,
    ) -> A2AResult<()> {
        Ok(())
    }

    /// Clean up resources for a completed task (e.g. remove broadcast channels).
    /// Default implementation is a no-op.
    async fn cleanup_task(&self, _task_id: &str) -> A2AResult<()> {
        Ok(())
    }
}

use std::pin::Pin;

use futures::Stream;

use crate::a2a::domain::*;

use super::task_manager::A2AResult;

/// Port for real-time streaming of task updates.
///
/// Supports pub/sub semantics: subscribers receive status and artifact events
/// as they occur, and broadcasters push events to all active subscribers.
#[async_trait::async_trait]
pub trait A2AStreamingHandler: Send + Sync {
    /// Subscribe to status updates for a task
    async fn subscribe_status(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<TaskStatusUpdateEvent>> + Send>>>;

    /// Subscribe to artifact updates for a task
    async fn subscribe_artifacts(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<TaskArtifactUpdateEvent>> + Send>>>;

    /// Subscribe to all update events (status + artifact) for a task
    async fn subscribe_all(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>>;

    /// Broadcast a status update to all subscribers of a task
    async fn broadcast_status(
        &self,
        task_id: &str,
        update: TaskStatusUpdateEvent,
    ) -> A2AResult<()>;

    /// Broadcast an artifact update to all subscribers of a task
    async fn broadcast_artifact(
        &self,
        task_id: &str,
        update: TaskArtifactUpdateEvent,
    ) -> A2AResult<()>;
}

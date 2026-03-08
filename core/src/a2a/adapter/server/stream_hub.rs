use std::collections::HashMap;
use std::pin::Pin;

use futures::Stream;
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::a2a::domain::*;
use crate::a2a::port::{A2AResult, A2AStreamingHandler};

const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Broadcast-based streaming hub for A2A task update events.
///
/// Uses `tokio::sync::broadcast` channels to support multiple concurrent
/// subscribers per task. Channels are lazily created on first access and
/// can be cleaned up via `remove_channel` after task completion.
pub struct StreamHub {
    channels: RwLock<HashMap<String, broadcast::Sender<UpdateEvent>>>,
    capacity: usize,
}

impl StreamHub {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            capacity,
        }
    }

    /// Get or create a broadcast sender for a task.
    ///
    /// Uses a read-lock fast path, upgrading to write-lock only when
    /// the channel does not yet exist.
    async fn get_or_create_sender(&self, task_id: &str) -> broadcast::Sender<UpdateEvent> {
        // Fast path: read lock
        {
            let channels = self.channels.read().await;
            if let Some(sender) = channels.get(task_id) {
                return sender.clone();
            }
        }
        // Slow path: write lock to create
        let mut channels = self.channels.write().await;
        channels
            .entry(task_id.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }

    /// Remove a task's broadcast channel (call after task completion).
    pub async fn remove_channel(&self, task_id: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(task_id);
    }
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl A2AStreamingHandler for StreamHub {
    async fn subscribe_status(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<TaskStatusUpdateEvent>> + Send>>> {
        let sender = self.get_or_create_sender(task_id).await;
        let receiver = sender.subscribe();
        let task_id_owned = task_id.to_string();

        let stream = BroadcastStream::new(receiver).filter_map(move |result| match result {
            Ok(UpdateEvent::StatusUpdate(event)) => Some(Ok(event)),
            Ok(_) => None, // Skip artifact events
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(task_id = %task_id_owned, skipped = n, "Status subscriber lagged");
                None
            }
        });

        Ok(Box::pin(stream))
    }

    async fn subscribe_artifacts(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<TaskArtifactUpdateEvent>> + Send>>> {
        let sender = self.get_or_create_sender(task_id).await;
        let receiver = sender.subscribe();
        let task_id_owned = task_id.to_string();

        let stream = BroadcastStream::new(receiver).filter_map(move |result| match result {
            Ok(UpdateEvent::ArtifactUpdate(event)) => Some(Ok(event)),
            Ok(_) => None, // Skip status events
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(task_id = %task_id_owned, skipped = n, "Artifact subscriber lagged");
                None
            }
        });

        Ok(Box::pin(stream))
    }

    async fn subscribe_all(
        &self,
        task_id: &str,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
        let sender = self.get_or_create_sender(task_id).await;
        let receiver = sender.subscribe();
        let task_id_owned = task_id.to_string();

        let stream = BroadcastStream::new(receiver).filter_map(move |result| match result {
            Ok(event) => Some(Ok(event)),
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(task_id = %task_id_owned, skipped = n, "Subscriber lagged");
                None
            }
        });

        Ok(Box::pin(stream))
    }

    async fn broadcast_status(
        &self,
        task_id: &str,
        update: TaskStatusUpdateEvent,
    ) -> A2AResult<()> {
        let sender = self.get_or_create_sender(task_id).await;
        // Ignore SendError — no subscribers is OK
        let _ = sender.send(UpdateEvent::StatusUpdate(update));
        Ok(())
    }

    async fn broadcast_artifact(
        &self,
        task_id: &str,
        update: TaskArtifactUpdateEvent,
    ) -> A2AResult<()> {
        let sender = self.get_or_create_sender(task_id).await;
        let _ = sender.send(UpdateEvent::ArtifactUpdate(update));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::message::{Artifact, Part};
    use crate::a2a::domain::task::{TaskState, TaskStatus};
    use chrono::Utc;
    use tokio_stream::StreamExt;

    fn make_status_event(task_id: &str, state: TaskState, is_final: bool) -> TaskStatusUpdateEvent {
        TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx-1".to_string(),
            status: TaskStatus {
                state,
                message: None,
                timestamp: Utc::now(),
            },
            is_final,
            metadata: None,
        }
    }

    fn make_artifact_event(task_id: &str) -> TaskArtifactUpdateEvent {
        TaskArtifactUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx-1".to_string(),
            artifact: Artifact {
                artifact_id: "art-1".to_string(),
                kind: "text".to_string(),
                parts: vec![Part::Text {
                    text: "hello".to_string(),
                    metadata: None,
                }],
                metadata: None,
            },
            append: false,
            last_chunk: true,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn subscribe_then_broadcast_receives_event() {
        let hub = StreamHub::new();
        let mut stream = hub.subscribe_all("task-1").await.unwrap();

        let event = make_status_event("task-1", TaskState::Working, false);
        hub.broadcast_status("task-1", event).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        match received {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.task_id, "task-1");
                assert_eq!(e.status.state, TaskState::Working);
            }
            _ => panic!("Expected StatusUpdate"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let hub = StreamHub::new();
        let mut stream1 = hub.subscribe_all("task-1").await.unwrap();
        let mut stream2 = hub.subscribe_all("task-1").await.unwrap();

        let event = make_status_event("task-1", TaskState::Working, false);
        hub.broadcast_status("task-1", event).await.unwrap();

        let r1 = stream1.next().await.unwrap().unwrap();
        let r2 = stream2.next().await.unwrap().unwrap();
        match (&r1, &r2) {
            (UpdateEvent::StatusUpdate(e1), UpdateEvent::StatusUpdate(e2)) => {
                assert_eq!(e1.task_id, e2.task_id);
            }
            _ => panic!("Expected StatusUpdate from both"),
        }
    }

    #[tokio::test]
    async fn broadcast_with_no_subscribers_no_error() {
        let hub = StreamHub::new();
        let event = make_status_event("task-1", TaskState::Working, false);
        // Should not error even with no subscribers
        let result = hub.broadcast_status("task-1", event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscribe_status_filters_out_artifact_events() {
        let hub = StreamHub::new();
        let mut stream = hub.subscribe_status("task-1").await.unwrap();

        // Broadcast an artifact event — should be filtered out
        let artifact = make_artifact_event("task-1");
        hub.broadcast_artifact("task-1", artifact).await.unwrap();

        // Broadcast a status event — should come through
        let status = make_status_event("task-1", TaskState::Completed, true);
        hub.broadcast_status("task-1", status).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received.task_id, "task-1");
        assert_eq!(received.status.state, TaskState::Completed);
        assert!(received.is_final);
    }

    #[tokio::test]
    async fn subscribe_artifacts_filters_out_status_events() {
        let hub = StreamHub::new();
        let mut stream = hub.subscribe_artifacts("task-1").await.unwrap();

        // Broadcast a status event — should be filtered out
        let status = make_status_event("task-1", TaskState::Working, false);
        hub.broadcast_status("task-1", status).await.unwrap();

        // Broadcast an artifact event — should come through
        let artifact = make_artifact_event("task-1");
        hub.broadcast_artifact("task-1", artifact).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        assert_eq!(received.task_id, "task-1");
        assert_eq!(received.artifact.artifact_id, "art-1");
        assert!(received.last_chunk);
    }

    #[tokio::test]
    async fn remove_channel_cleans_up() {
        let hub = StreamHub::new();

        // Create a channel by subscribing
        let _stream = hub.subscribe_all("task-1").await.unwrap();

        {
            let channels = hub.channels.read().await;
            assert!(channels.contains_key("task-1"));
        }

        hub.remove_channel("task-1").await;

        {
            let channels = hub.channels.read().await;
            assert!(!channels.contains_key("task-1"));
        }
    }

    #[tokio::test]
    async fn channel_lazily_created_on_first_access() {
        let hub = StreamHub::new();

        // No channels yet
        {
            let channels = hub.channels.read().await;
            assert!(channels.is_empty());
        }

        // Subscribe creates the channel
        let _stream = hub.subscribe_all("task-1").await.unwrap();

        {
            let channels = hub.channels.read().await;
            assert!(channels.contains_key("task-1"));
        }
    }

    #[tokio::test]
    async fn with_capacity_sets_custom_capacity() {
        let hub = StreamHub::with_capacity(64);
        assert_eq!(hub.capacity, 64);
    }
}

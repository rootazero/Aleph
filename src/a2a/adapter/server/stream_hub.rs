use std::collections::HashMap;
use std::pin::Pin;

use futures::Stream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::a2a::domain::{A2AError, TaskStatusUpdateEvent, UpdateEvent};
use crate::a2a::port::{A2AResult, A2AStreamingHandler};
use crate::a2a::service::notification::NotificationService;
use crate::sync_primitives::{Arc, AsyncRwLock};

const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Broadcast-based streaming hub for A2A task update events.
///
/// Uses `tokio::sync::broadcast` channels to support multiple concurrent
/// subscribers per task. Channels are lazily created on first access and
/// can be cleaned up via `remove_channel` after task completion.
///
/// When constructed with a [`NotificationService`], every broadcast also fans
/// the event out to registered push-notification webhooks (fire-and-forget),
/// so clients that registered a push config via
/// `tasks/pushNotificationConfig/set` but are not attached to the SSE stream
/// still receive task updates.
pub struct StreamHub {
    channels: AsyncRwLock<HashMap<String, broadcast::Sender<UpdateEvent>>>,
    capacity: usize,
    /// Optional push-notification sink. When `Some`, broadcasts are also
    /// delivered to webhooks registered for the task.
    notification: Option<Arc<NotificationService>>,
}

impl StreamHub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: AsyncRwLock::new(HashMap::new()),
            capacity: DEFAULT_CHANNEL_CAPACITY,
            notification: None,
        }
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        // broadcast::channel panics if capacity is 0
        let capacity = capacity.max(1);
        Self {
            channels: AsyncRwLock::new(HashMap::new()),
            capacity,
            notification: None,
        }
    }

    /// Build a hub that also delivers every broadcast to push-notification
    /// webhooks via the shared [`NotificationService`].
    #[must_use]
    pub fn with_notification(notification: Arc<NotificationService>) -> Self {
        Self {
            channels: AsyncRwLock::new(HashMap::new()),
            capacity: DEFAULT_CHANNEL_CAPACITY,
            notification: Some(notification),
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
        // Fan out to push-notification webhooks before moving `update` into the
        // SSE channel. Spawned fire-and-forget so a slow or unreachable webhook
        // never stalls SSE delivery or the calling bridge.
        if let Some(notification) = self.notification.clone() {
            let task_id_owned = task_id.to_string();
            let event = update.clone();
            tokio::spawn(async move {
                notification
                    .notify_status_update(&task_id_owned, &event)
                    .await;
            });
        }
        let sender = self.get_or_create_sender(task_id).await;
        let is_terminal = update.is_final;
        match sender.send(UpdateEvent::StatusUpdate(update)) {
            Ok(_n) => {}
            Err(tokio::sync::broadcast::error::SendError(_event)) => {
                if is_terminal {
                    // A terminal event with no subscribers (or all
                    // subscribers lagged) silently turns `fold_stream`'s
                    // `success=true` predicate into `success=false` — the
                    // caller sees a "successful" task marked failed. Log
                    // loudly so this is at least diagnosable, and return
                    // Err so the bridge can choose to persist the
                    // terminal state for late subscribers.
                    tracing::error!(
                        task_id = %task_id,
                        "broadcast_status: terminal event dropped (no subscribers or all lagged); \
                         downstream SSE consumers will see the task as not-finished"
                    );
                    return Err(A2AError::InternalError(format!(
                        "terminal event for task {task_id} dropped: no live subscribers"
                    )));
                } else {
                    tracing::debug!(
                        task_id = %task_id,
                        "broadcast_status: non-terminal event dropped (no subscribers)"
                    );
                }
            }
        }
        Ok(())
    }

    async fn cleanup_task(&self, task_id: &str) -> A2AResult<()> {
        self.remove_channel(task_id).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn with_notification_still_delivers_sse() {
        use crate::sync_primitives::Arc;
        // No push config registered → notify_* is a cheap no-op (no webhook
        // POST), so this exercises the fan-out path without network access.
        let hub = StreamHub::with_notification(Arc::new(NotificationService::new()));
        let mut stream = hub.subscribe_all("task-1").await.unwrap();

        let event = make_status_event("task-1", TaskState::Completed, true);
        hub.broadcast_status("task-1", event).await.unwrap();

        let received = stream.next().await.unwrap().unwrap();
        match received {
            UpdateEvent::StatusUpdate(e) => {
                assert_eq!(e.task_id, "task-1");
                assert_eq!(e.status.state, TaskState::Completed);
            }
            _ => panic!("Expected StatusUpdate"),
        }
    }
}

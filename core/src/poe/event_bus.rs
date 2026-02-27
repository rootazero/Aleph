//! POE event bus — broadcast channel for domain events.

use tokio::sync::broadcast;
use tracing::debug;

use super::events::PoeEventEnvelope;

const DEFAULT_CAPACITY: usize = 1024;

/// Broadcast-based event bus for POE domain events.
///
/// Wraps a `tokio::sync::broadcast` channel. Events emitted via [`emit()`](Self::emit)
/// are delivered to all active subscribers. If no subscribers exist, events are silently
/// dropped (not an error — projectors may connect later).
#[derive(Debug, Clone)]
pub struct PoeEventBus {
    sender: broadcast::Sender<PoeEventEnvelope>,
}

impl PoeEventBus {
    /// Create a new event bus with the given buffer capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Emit an event to all active subscribers.
    ///
    /// Silently drops the event if no subscribers are connected.
    pub fn emit(&self, envelope: PoeEventEnvelope) {
        let event_type = envelope.event.event_type_tag();
        let _ = self.sender.send(envelope);
        debug!("POE event emitted: {}", event_type);
    }

    /// Subscribe to receive future events.
    pub fn subscribe(&self) -> broadcast::Receiver<PoeEventEnvelope> {
        self.sender.subscribe()
    }

    /// Get the number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for PoeEventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};

    #[tokio::test]
    async fn test_emit_and_receive() {
        let bus = PoeEventBus::new(64);
        let mut rx = bus.subscribe();

        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "t1".into(),
                objective: "test".into(),
                hard_constraints_count: 1,
                soft_metrics_count: 0,
            },
            None,
        );

        bus.emit(envelope.clone());
        let received = rx.recv().await.unwrap();
        assert_eq!(received.task_id, "t1");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = PoeEventBus::new(64);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "t1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.1,
            },
            None,
        );

        bus.emit(envelope);
        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1.task_id, r2.task_id);
    }

    #[test]
    fn test_default_capacity() {
        let bus = PoeEventBus::default();
        let _rx = bus.subscribe();
    }

    #[test]
    fn test_emit_without_receivers() {
        let bus = PoeEventBus::new(64);
        // Should NOT panic or error
        bus.emit(PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OperationAttempted {
                task_id: "t1".into(),
                attempt: 1,
                tokens_used: 100,
            },
            None,
        ));
    }
}

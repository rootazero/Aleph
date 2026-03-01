//! Daemon Event Bus
//!
//! Pub/sub event distribution using tokio::sync::broadcast.

use crate::daemon::{DaemonError, DaemonEvent, Result};
use tokio::sync::broadcast;
use tracing::{debug, warn};

/// Event bus for daemon events
#[derive(Debug, Clone)]
pub struct DaemonEventBus {
    sender: broadcast::Sender<DaemonEvent>,
}

impl DaemonEventBus {
    /// Create a new event bus with the given capacity
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of buffered events
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Send an event to all subscribers
    ///
    /// Returns Ok(()) even when there are no receivers (this is normal during
    /// startup before subscribers have registered). Only returns Err for
    /// actual send failures.
    pub fn send(&self, event: DaemonEvent) -> Result<()> {
        match self.sender.send(event.clone()) {
            Ok(receiver_count) => {
                debug!("Event sent to {} receivers: {:?}", receiver_count, event);
                Ok(())
            }
            Err(_) => {
                // No active receivers is not an error — it's expected during startup
                // before subscribers register. Treating it as an error would kill the
                // WorldModel event loop permanently.
                warn!("No active receivers for event: {:?}", event);
                Ok(())
            }
        }
    }

    /// Subscribe to events
    ///
    /// Returns a receiver that will receive all future events
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.sender.subscribe()
    }

    /// Get the number of active subscribers
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::RawEvent;
    use chrono::Utc;

    #[tokio::test]
    async fn test_event_bus_basic() {
        let bus = DaemonEventBus::new(10);
        let mut rx = bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        bus.send(event.clone()).unwrap();
        let received = rx.recv().await.unwrap();

        assert!(matches!(
            received,
            DaemonEvent::Raw(RawEvent::Heartbeat { .. })
        ));
    }

    #[tokio::test]
    async fn test_event_bus_no_receivers() {
        let bus = DaemonEventBus::new(10);

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        // send() returns Ok even with no receivers (graceful handling during startup)
        let result = bus.send(event);
        assert!(result.is_ok());
    }

    #[test]
    fn test_receiver_count() {
        let bus = DaemonEventBus::new(10);
        assert_eq!(bus.receiver_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 2);
    }
}

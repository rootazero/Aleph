#[cfg(test)]
mod tests {
    use crate::daemon::{DaemonEvent, DaemonEventBus, RawEvent};
    use chrono::Utc;

    #[tokio::test]
    async fn test_event_bus_send_and_receive() {
        let bus = DaemonEventBus::new(10);
        let mut receiver = bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        bus.send(event.clone()).unwrap();
        let received = receiver.recv().await.unwrap();

        assert!(matches!(received, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
    }

    #[tokio::test]
    async fn test_event_bus_multiple_subscribers() {
        let bus = DaemonEventBus::new(10);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        bus.send(event.clone()).unwrap();

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();

        assert!(matches!(r1, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
        assert!(matches!(r2, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
    }
}

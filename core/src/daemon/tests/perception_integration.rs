#[cfg(test)]
mod tests {
    use crate::daemon::{
        DaemonEvent, DaemonEventBus, PerceptionConfig, RawEvent, WatcherRegistry,
    };
    use crate::daemon::perception::watchers::{ProcessWatcher, TimeWatcher};
    use crate::sync_primitives::Arc;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    #[ignore] // Manual test - requires time to collect events
    async fn test_perception_full_lifecycle() {
        // Create minimal config
        let mut config = PerceptionConfig::default();
        config.time.heartbeat_interval_secs = 1; // Fast heartbeat
        config.process.poll_interval_secs = 2;
        config.filesystem.enabled = false; // Disable to avoid noise
        config.system.enabled = false;

        // Create EventBus and registry
        let bus = Arc::new(DaemonEventBus::new(100));
        let mut registry = WatcherRegistry::new();

        // Register watchers
        registry.register(Arc::new(TimeWatcher::new(config.time.clone())));
        registry.register(Arc::new(ProcessWatcher::new(config.process.clone())));

        // Start watchers
        registry.start_all(bus.clone()).await.unwrap();

        // Subscribe to events
        let mut receiver = bus.subscribe();

        // Collect events for 3 seconds
        let result = timeout(Duration::from_secs(3), async {
            let mut event_count = 0;
            let mut heartbeat_count = 0;

            while event_count < 5 {
                if let Ok(event) = receiver.recv().await {
                    event_count += 1;
                    if matches!(event, DaemonEvent::Raw(RawEvent::Heartbeat { .. })) {
                        heartbeat_count += 1;
                    }
                }
            }

            (event_count, heartbeat_count)
        })
        .await;

        assert!(result.is_ok());
        let (total, heartbeats) = result.unwrap();
        assert!(total >= 5);
        assert!(heartbeats >= 2); // At least 2 heartbeats in 3 seconds

        // Shutdown
        registry.shutdown_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_bus_capacity_limit() {
        let bus = DaemonEventBus::new(10);

        // Test that bus can handle the configured capacity
        let mut receiver = bus.subscribe();

        // Send events up to capacity
        for _i in 0..10 {
            let event = DaemonEvent::Raw(RawEvent::Heartbeat {
                timestamp: chrono::Utc::now(),
            });
            bus.send(event).expect("Should send within capacity");
        }

        // Verify we can receive all events
        let mut received = 0;
        while receiver.try_recv().is_ok() {
            received += 1;
        }

        assert_eq!(received, 10, "Should receive all events within capacity");

        // Test that exceeding capacity works (broadcast channels allow overflow)
        for _i in 0..5 {
            let event = DaemonEvent::Raw(RawEvent::Heartbeat {
                timestamp: chrono::Utc::now(),
            });
            bus.send(event).expect("Broadcast channels allow overflow");
        }

        // Additional events should also be receivable
        let mut additional = 0;
        while receiver.try_recv().is_ok() {
            additional += 1;
        }

        assert_eq!(additional, 5, "Should receive additional events");
    }
}

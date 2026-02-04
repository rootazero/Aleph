#[cfg(test)]
mod tests {
    use crate::daemon::{
        DaemonEvent, DaemonEventBus, RawEvent,
        perception::{TimeWatcher, TimeWatcherConfig, Watcher, WatcherControl},
    };
    use std::sync::Arc;
    use tokio::sync::watch;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_time_watcher_heartbeat() {
        let config = TimeWatcherConfig {
            enabled: true,
            heartbeat_interval_secs: 1, // Fast for testing
        };

        let watcher = TimeWatcher::new(config);
        let bus = Arc::new(DaemonEventBus::new(10));
        let mut receiver = bus.subscribe();

        let (tx, rx) = watch::channel(WatcherControl::Run);

        // Start watcher in background
        let watcher_task = tokio::spawn({
            let bus = bus.clone();
            async move {
                watcher.run(bus, rx).await
            }
        });

        // Wait for first heartbeat
        let result = timeout(Duration::from_secs(2), receiver.recv()).await;
        assert!(result.is_ok());

        let event = result.unwrap().unwrap();
        assert!(matches!(event, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));

        // Shutdown
        tx.send(WatcherControl::Shutdown).unwrap();
        let _ = watcher_task.await;
    }

    #[tokio::test]
    async fn test_time_watcher_is_not_pausable() {
        let config = TimeWatcherConfig {
            enabled: true,
            heartbeat_interval_secs: 30,
        };

        let watcher = TimeWatcher::new(config);
        assert_eq!(watcher.id(), "time");
        assert!(!watcher.is_pausable()); // Level 0
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::{DaemonEventBus, perception::{WatcherRegistry, WatcherControl, WatcherHealth}};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_registry_lifecycle() {
        let mut registry = WatcherRegistry::new();
        let bus = Arc::new(DaemonEventBus::new(10));

        // Registry starts empty
        assert_eq!(registry.watcher_count(), 0);

        // Start and shutdown without watchers should work
        registry.start_all(bus.clone()).await.unwrap();
        registry.shutdown_all().await.unwrap();
    }
}

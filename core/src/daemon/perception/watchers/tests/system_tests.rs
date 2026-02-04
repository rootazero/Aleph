#[cfg(test)]
mod tests {
    use crate::daemon::perception::{SystemStateWatcher, SystemWatcherConfig, Watcher};

    #[test]
    fn test_system_watcher_creation() {
        let config = SystemWatcherConfig {
            enabled: true,
            poll_interval_secs: 60,
            track_battery: true,
            track_network: true,
            idle_threshold_secs: 300,
        };

        let watcher = SystemStateWatcher::new(config);
        assert_eq!(watcher.id(), "system");
        assert!(!watcher.is_pausable()); // Level 0
    }
}

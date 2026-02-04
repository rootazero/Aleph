#[cfg(test)]
mod tests {
    use crate::daemon::perception::{ProcessWatcher, ProcessWatcherConfig, Watcher};

    #[test]
    fn test_process_watcher_creation() {
        let config = ProcessWatcherConfig {
            enabled: true,
            poll_interval_secs: 5,
            watched_apps: vec!["Code".to_string()],
        };

        let watcher = ProcessWatcher::new(config);
        assert_eq!(watcher.id(), "process");
        assert!(!watcher.is_pausable()); // Level 0
    }
}

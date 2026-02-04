#[cfg(test)]
mod tests {
    use crate::daemon::perception::{FSEventWatcher, FSWatcherConfig, Watcher};

    #[test]
    fn test_fs_watcher_creation() {
        let config = FSWatcherConfig {
            enabled: true,
            watched_paths: vec!["/tmp".to_string()],
            ignore_patterns: vec!["**/.git/**".to_string()],
            debounce_ms: 500,
        };

        let watcher = FSEventWatcher::new(config);
        assert_eq!(watcher.id(), "filesystem");
        assert!(watcher.is_pausable()); // Level 1
    }
}

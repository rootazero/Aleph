#[cfg(test)]
mod tests {
    use crate::daemon::perception::PerceptionConfig;
    use std::path::PathBuf;

    #[test]
    fn test_default_config() {
        let config = PerceptionConfig::default();

        assert!(config.enabled);
        assert!(config.process.enabled);
        assert_eq!(config.process.poll_interval_secs, 5);
        assert!(config.process.watched_apps.contains(&"Code".to_string()));

        assert!(config.filesystem.enabled);
        assert_eq!(config.filesystem.debounce_ms, 500);

        assert!(config.time.enabled);
        assert_eq!(config.time.heartbeat_interval_secs, 30);

        assert!(config.system.enabled);
        assert_eq!(config.system.poll_interval_secs, 60);
        assert!(config.system.track_battery);
    }

    #[test]
    fn test_config_serialization() {
        let config = PerceptionConfig::default();
        let toml_str = toml::to_string(&config).unwrap();

        assert!(toml_str.contains("enabled = true"));
        assert!(toml_str.contains("[process]"));
        assert!(toml_str.contains("[filesystem]"));
    }
}

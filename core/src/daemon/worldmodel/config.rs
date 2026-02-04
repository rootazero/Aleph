//! WorldModel Configuration

use serde::Deserialize;
use std::path::PathBuf;

/// WorldModel configuration
#[derive(Debug, Clone, Deserialize)]
pub struct WorldModelConfig {
    /// State file path (default: ~/.aleph/worldmodel_state.json)
    pub state_path: Option<PathBuf>,

    /// Batch processing interval (seconds)
    #[serde(default = "default_batch_interval")]
    pub batch_interval: u64,

    /// Periodic inference interval (seconds)
    #[serde(default = "default_periodic_interval")]
    pub periodic_interval: u64,

    /// InferenceCache buffer size
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,

    /// Activity inference confidence threshold
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

fn default_batch_interval() -> u64 {
    5
}

fn default_periodic_interval() -> u64 {
    30
}

fn default_cache_size() -> usize {
    100
}

fn default_confidence_threshold() -> f64 {
    0.7
}

impl Default for WorldModelConfig {
    fn default() -> Self {
        Self {
            state_path: None,
            batch_interval: default_batch_interval(),
            periodic_interval: default_periodic_interval(),
            cache_size: default_cache_size(),
            confidence_threshold: default_confidence_threshold(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WorldModelConfig::default();
        assert_eq!(config.batch_interval, 5);
        assert_eq!(config.periodic_interval, 30);
        assert_eq!(config.cache_size, 100);
        assert_eq!(config.confidence_threshold, 0.7);
        assert!(config.state_path.is_none());
    }

    #[test]
    fn test_config_deserialization() {
        let toml = r#"
            batch_interval = 10
            periodic_interval = 60
            cache_size = 200
            confidence_threshold = 0.8
        "#;

        let config: WorldModelConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.batch_interval, 10);
        assert_eq!(config.periodic_interval, 60);
        assert_eq!(config.cache_size, 200);
        assert_eq!(config.confidence_threshold, 0.8);
    }

    #[test]
    fn test_partial_config_uses_defaults() {
        let toml = r#"
            batch_interval = 15
        "#;

        let config: WorldModelConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.batch_interval, 15);
        assert_eq!(config.periodic_interval, 30); // default
        assert_eq!(config.cache_size, 100); // default
    }
}

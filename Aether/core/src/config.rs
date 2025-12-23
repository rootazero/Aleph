/// Configuration structure for Aether
///
/// Phase 1: Stub implementation with basic fields.
/// Phase 4 will add TOML parsing and full configuration support.
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default hotkey (hardcoded to "Command+Grave" in Phase 1)
    pub default_hotkey: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_hotkey: "Command+Grave".to_string(),
        }
    }
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_hotkey, "Command+Grave");
    }

    #[test]
    fn test_new_config() {
        let config = Config::new();
        assert_eq!(config.default_hotkey, "Command+Grave");
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("Command+Grave"));
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{"default_hotkey":"Command+Grave"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.default_hotkey, "Command+Grave");
    }
}

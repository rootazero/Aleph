//! Typo correction configuration types
//!
//! Configuration for the quick typo correction feature that allows
//! users to correct text errors with a double-space shortcut.

use serde::{Deserialize, Serialize};

/// Typo correction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypoCorrectionConfig {
    /// Whether typo correction is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Provider name to use for correction (e.g., "openai", "gemini")
    /// Must match a key in [providers] section
    #[serde(default)]
    pub provider: Option<String>,

    /// Optional model override (e.g., "gpt-4o-mini")
    /// If not specified, uses the provider's default model
    #[serde(default)]
    pub model: Option<String>,

    /// Timeout in seconds for correction requests
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Maximum text length to process (characters)
    #[serde(default = "default_max_length")]
    pub max_length: usize,
}

fn default_timeout() -> u64 {
    5
}

fn default_max_length() -> usize {
    2000
}

impl Default for TypoCorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            model: None,
            timeout_seconds: default_timeout(),
            max_length: default_max_length(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TypoCorrectionConfig::default();
        assert!(!config.enabled);
        assert!(config.provider.is_none());
        assert!(config.model.is_none());
        assert_eq!(config.timeout_seconds, 5);
        assert_eq!(config.max_length, 2000);
    }

    #[test]
    fn test_deserialize_config() {
        let toml = r#"
            enabled = true
            provider = "openai"
            model = "gpt-4o-mini"
            timeout_seconds = 3
            max_length = 1000
        "#;

        let config: TypoCorrectionConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.provider, Some("openai".to_string()));
        assert_eq!(config.model, Some("gpt-4o-mini".to_string()));
        assert_eq!(config.timeout_seconds, 3);
        assert_eq!(config.max_length, 1000);
    }

    #[test]
    fn test_deserialize_minimal_config() {
        let toml = r#"
            enabled = true
            provider = "gemini"
        "#;

        let config: TypoCorrectionConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.provider, Some("gemini".to_string()));
        assert!(config.model.is_none());
        assert_eq!(config.timeout_seconds, 5); // default
        assert_eq!(config.max_length, 2000); // default
    }
}

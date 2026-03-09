//! Media pipeline configuration
//!
//! Contains settings for the media understanding pipeline:
//! - MediaConfig: Top-level toggle and policy settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::media::policy::MediaPolicy;

/// Media understanding pipeline configuration.
///
/// ```toml
/// [media]
/// enabled = true
///
/// [media.policy]
/// max_image_bytes = 20971520
/// max_audio_bytes = 104857600
/// max_video_duration = 1800
/// max_document_pages = 200
/// temp_ttl_secs = 3600
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaConfig {
    /// Whether the media pipeline is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Size and lifecycle policy.
    #[serde(default)]
    pub policy: MediaPolicy,
}

fn default_true() -> bool {
    true
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: MediaPolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_config_defaults() {
        let config = MediaConfig::default();
        assert!(config.enabled);
        assert_eq!(config.policy.max_image_bytes, 20 * 1024 * 1024);
    }

    #[test]
    fn media_config_serde_round_trip() {
        let config = MediaConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        let rt: MediaConfig = serde_json::from_value(json).unwrap();
        assert!(rt.enabled);
        assert_eq!(rt.policy.max_image_bytes, config.policy.max_image_bytes);
    }

    #[test]
    fn media_config_from_toml_defaults() {
        let toml_str = "";
        let config: MediaConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.policy.max_video_duration, 1800);
    }

    #[test]
    fn media_config_from_toml_disabled() {
        let toml_str = r#"enabled = false"#;
        let config: MediaConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
    }

    #[test]
    fn media_config_from_toml_custom_policy() {
        let toml_str = r#"
        [policy]
        max_image_bytes = 10485760
        max_document_pages = 50
        "#;
        let config: MediaConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.policy.max_image_bytes, 10 * 1024 * 1024);
        assert_eq!(config.policy.max_document_pages, 50);
    }
}

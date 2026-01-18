//! Intent detection policies
//!
//! Configurable thresholds and patterns for AI-based intent detection.

use serde::{Deserialize, Serialize};

/// Policy for AI-based intent detection
///
/// Controls confidence thresholds, timeouts, and URL pattern matching
/// for intent classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentDetectionPolicy {
    /// Minimum confidence threshold for accepting an intent (0.0-1.0)
    /// Default: 0.7
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,

    /// Timeout for AI intent detection in milliseconds
    /// Default: 3000
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Minimum input length for intent detection (skip shorter inputs)
    /// Default: 3
    #[serde(default = "default_min_input_length")]
    pub min_input_length: u64,

    /// URL patterns for video detection
    /// Default: ["youtube.com/watch", "youtu.be/", "youtube.com/shorts",
    ///           "bilibili.com/video", "b23.tv/"]
    #[serde(default = "default_video_url_patterns")]
    pub video_url_patterns: Vec<String>,
}

impl Default for IntentDetectionPolicy {
    fn default() -> Self {
        Self {
            confidence_threshold: default_confidence_threshold(),
            timeout_ms: default_timeout_ms(),
            min_input_length: default_min_input_length(),
            video_url_patterns: default_video_url_patterns(),
        }
    }
}

fn default_confidence_threshold() -> f64 {
    0.7
}

fn default_timeout_ms() -> u64 {
    3000
}

fn default_min_input_length() -> u64 {
    3
}

fn default_video_url_patterns() -> Vec<String> {
    vec![
        "youtube.com/watch".to_string(),
        "youtu.be/".to_string(),
        "youtube.com/shorts".to_string(),
        "bilibili.com/video".to_string(),
        "b23.tv/".to_string(),
    ]
}

impl IntentDetectionPolicy {
    /// Check if the confidence threshold is valid (0.0-1.0)
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.confidence_threshold)
    }

    /// Get timeout as std::time::Duration
    pub fn timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = IntentDetectionPolicy::default();
        assert_eq!(policy.confidence_threshold, 0.7);
        assert_eq!(policy.timeout_ms, 3000);
        assert_eq!(policy.min_input_length, 3);
        assert!(policy
            .video_url_patterns
            .contains(&"youtube.com/watch".to_string()));
    }

    #[test]
    fn test_validity_check() {
        let mut policy = IntentDetectionPolicy::default();
        assert!(policy.is_valid());

        policy.confidence_threshold = 1.5;
        assert!(!policy.is_valid());

        policy.confidence_threshold = -0.1;
        assert!(!policy.is_valid());
    }

    #[test]
    fn test_timeout_duration() {
        let policy = IntentDetectionPolicy::default();
        assert_eq!(
            policy.timeout_duration(),
            std::time::Duration::from_millis(3000)
        );
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            confidence_threshold = 0.8
            timeout_ms = 5000
        "#;
        let policy: IntentDetectionPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.confidence_threshold, 0.8);
        assert_eq!(policy.timeout_ms, 5000);
        // Defaults for unspecified fields
        assert_eq!(policy.min_input_length, 3);
        assert!(policy
            .video_url_patterns
            .contains(&"youtube.com/watch".to_string()));
    }
}

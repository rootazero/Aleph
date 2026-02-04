//! Dispatcher Configuration

use serde::Deserialize;

/// Dispatcher configuration
#[derive(Debug, Clone, Deserialize)]
pub struct DispatcherConfig {
    /// Enable dispatcher (default: true)
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Idle threshold in seconds (default: 30)
    #[serde(default = "default_idle_threshold")]
    pub idle_threshold: u64,

    /// High-risk action expiry in hours (default: 24)
    #[serde(default = "default_high_risk_expiry")]
    pub high_risk_expiry_hours: u64,

    /// Medium-risk action expiry in hours (default: 12)
    #[serde(default = "default_medium_risk_expiry")]
    pub medium_risk_expiry_hours: u64,

    /// Auto-execute low-risk actions without confirmation (default: true)
    #[serde(default = "default_auto_execute_low_risk")]
    pub auto_execute_low_risk: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_idle_threshold() -> u64 {
    30
}

fn default_high_risk_expiry() -> u64 {
    24
}

fn default_medium_risk_expiry() -> u64 {
    12
}

fn default_auto_execute_low_risk() -> bool {
    true
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            idle_threshold: default_idle_threshold(),
            high_risk_expiry_hours: default_high_risk_expiry(),
            medium_risk_expiry_hours: default_medium_risk_expiry(),
            auto_execute_low_risk: default_auto_execute_low_risk(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = DispatcherConfig::default();
        assert!(config.enabled);
        assert_eq!(config.idle_threshold, 30);
        assert_eq!(config.high_risk_expiry_hours, 24);
        assert_eq!(config.medium_risk_expiry_hours, 12);
        assert!(config.auto_execute_low_risk);
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{
            "enabled": false,
            "idle_threshold": 60
        }"#;
        let config: DispatcherConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.idle_threshold, 60);
        // Other fields should use defaults
        assert_eq!(config.high_risk_expiry_hours, 24);
    }
}

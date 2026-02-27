//! Signal Channel Configuration
//!
//! Configuration types for the Signal integration using signal-cli's REST API.

use serde::{Deserialize, Serialize};

fn default_api_url() -> String {
    "http://localhost:8080".to_string()
}

fn default_poll_interval() -> u64 {
    2
}

fn default_true() -> bool {
    true
}

/// Signal channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalConfig {
    /// URL of signal-cli REST API (e.g., "http://localhost:8080")
    #[serde(default = "default_api_url")]
    pub api_url: String,

    /// Registered phone number (e.g., "+1234567890")
    pub phone_number: String,

    /// Allowed phone numbers (empty = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            phone_number: String::new(),
            allowed_users: Vec::new(),
            poll_interval_secs: default_poll_interval(),
            send_typing: true,
        }
    }
}

impl SignalConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.phone_number.is_empty() {
            return Err("phone_number is required".to_string());
        }
        if !self.phone_number.starts_with('+') {
            return Err("phone_number must start with '+'".to_string());
        }
        if self.api_url.is_empty() {
            return Err("api_url is required".to_string());
        }
        Ok(())
    }

    /// Check if a phone number is allowed
    pub fn is_user_allowed(&self, phone: &str) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.iter().any(|u| u == phone)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SignalConfig::default();
        assert_eq!(config.api_url, "http://localhost:8080");
        assert!(config.phone_number.is_empty());
        assert!(config.allowed_users.is_empty());
        assert_eq!(config.poll_interval_secs, 2);
        assert!(config.send_typing);
    }

    #[test]
    fn test_validate_empty_phone() {
        let config = SignalConfig::default();
        let err = config.validate().unwrap_err();
        assert_eq!(err, "phone_number is required");
    }

    #[test]
    fn test_validate_phone_no_plus() {
        let config = SignalConfig {
            phone_number: "1234567890".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("+"),
            "Error should mention '+' requirement: {}",
            err
        );
    }

    #[test]
    fn test_validate_empty_api_url() {
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            api_url: String::new(),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), "api_url is required");
    }

    #[test]
    fn test_validate_valid_config() {
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_user_allowed_empty_list() {
        let config = SignalConfig::default();
        assert!(config.is_user_allowed("+1234567890"));
        assert!(config.is_user_allowed("+9876543210"));
    }

    #[test]
    fn test_user_allowed_with_list() {
        let config = SignalConfig {
            allowed_users: vec![
                "+1234567890".to_string(),
                "+1111111111".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.is_user_allowed("+1234567890"));
        assert!(config.is_user_allowed("+1111111111"));
        assert!(!config.is_user_allowed("+9999999999"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = SignalConfig {
            api_url: "http://signal:9080".to_string(),
            phone_number: "+1234567890".to_string(),
            allowed_users: vec!["+9876543210".to_string()],
            poll_interval_secs: 5,
            send_typing: false,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SignalConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.api_url, config.api_url);
        assert_eq!(deserialized.phone_number, config.phone_number);
        assert_eq!(deserialized.allowed_users, config.allowed_users);
        assert_eq!(deserialized.poll_interval_secs, config.poll_interval_secs);
        assert_eq!(deserialized.send_typing, config.send_typing);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"phone_number": "+1234567890"}"#;
        let config: SignalConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.api_url, "http://localhost:8080");
        assert_eq!(config.poll_interval_secs, 2);
        assert!(config.send_typing);
        assert!(config.allowed_users.is_empty());
    }
}

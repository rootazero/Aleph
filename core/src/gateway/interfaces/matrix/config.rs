//! Matrix Channel Configuration
//!
//! Configuration types for the Matrix Bot integration using the Client-Server API v3.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_sync_timeout() -> u64 {
    30000
}

/// Matrix channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g., "https://matrix.org")
    pub homeserver_url: String,

    /// Access token for authentication (Bearer token)
    pub access_token: String,

    /// Allowed room IDs (empty = allow all joined rooms)
    #[serde(default)]
    pub allowed_rooms: Vec<String>,

    /// Sync long-poll timeout in milliseconds
    #[serde(default = "default_sync_timeout")]
    pub sync_timeout_ms: u64,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            homeserver_url: String::new(),
            access_token: String::new(),
            allowed_rooms: Vec::new(),
            sync_timeout_ms: 30000,
            send_typing: true,
        }
    }
}

impl MatrixConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.homeserver_url.is_empty() {
            return Err("homeserver_url is required".to_string());
        }
        if !self.homeserver_url.starts_with("http") {
            return Err(
                "homeserver_url must start with 'http://' or 'https://'".to_string(),
            );
        }
        if self.access_token.is_empty() {
            return Err("access_token is required".to_string());
        }
        Ok(())
    }

    /// Check if a room ID is allowed
    pub fn is_room_allowed(&self, room_id: &str) -> bool {
        if self.allowed_rooms.is_empty() {
            true
        } else {
            self.allowed_rooms.contains(&room_id.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MatrixConfig::default();
        assert!(config.homeserver_url.is_empty());
        assert!(config.access_token.is_empty());
        assert!(config.allowed_rooms.is_empty());
        assert_eq!(config.sync_timeout_ms, 30000);
        assert!(config.send_typing);
    }

    #[test]
    fn test_validate_empty_homeserver() {
        let config = MatrixConfig::default();
        let err = config.validate().unwrap_err();
        assert_eq!(err, "homeserver_url is required");
    }

    #[test]
    fn test_validate_invalid_homeserver_scheme() {
        let config = MatrixConfig {
            homeserver_url: "matrix.org".to_string(),
            access_token: "token".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            err.contains("http"),
            "Error should mention http requirement: {}",
            err
        );
    }

    #[test]
    fn test_validate_empty_access_token() {
        let config = MatrixConfig {
            homeserver_url: "https://matrix.org".to_string(),
            access_token: String::new(),
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            "access_token is required"
        );
    }

    #[test]
    fn test_validate_valid_config() {
        let config = MatrixConfig {
            homeserver_url: "https://matrix.org".to_string(),
            access_token: "syt_abc123_defghi_JKLMNO".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_http_url() {
        let config = MatrixConfig {
            homeserver_url: "http://localhost:8008".to_string(),
            access_token: "token123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_room_allowed_empty_list() {
        let config = MatrixConfig::default();
        assert!(config.is_room_allowed("!room1:matrix.org"));
        assert!(config.is_room_allowed("!room2:example.com"));
    }

    #[test]
    fn test_room_allowed_with_list() {
        let config = MatrixConfig {
            allowed_rooms: vec![
                "!room1:matrix.org".to_string(),
                "!room2:matrix.org".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.is_room_allowed("!room1:matrix.org"));
        assert!(config.is_room_allowed("!room2:matrix.org"));
        assert!(!config.is_room_allowed("!room3:matrix.org"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = MatrixConfig {
            homeserver_url: "https://matrix.org".to_string(),
            access_token: "syt_token_abc123".to_string(),
            allowed_rooms: vec!["!room1:matrix.org".to_string()],
            sync_timeout_ms: 60000,
            send_typing: false,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MatrixConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.homeserver_url, config.homeserver_url);
        assert_eq!(deserialized.access_token, config.access_token);
        assert_eq!(deserialized.allowed_rooms, config.allowed_rooms);
        assert_eq!(deserialized.sync_timeout_ms, config.sync_timeout_ms);
        assert_eq!(deserialized.send_typing, config.send_typing);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"homeserver_url": "https://matrix.org", "access_token": "token123"}"#;
        let config: MatrixConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.sync_timeout_ms, 30000);
        assert!(config.send_typing);
        assert!(config.allowed_rooms.is_empty());
    }
}

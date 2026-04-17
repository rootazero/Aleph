//! Matrix Channel Configuration
//!
//! Configuration types for the Matrix Bot integration using the official Rust SDK.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}

fn default_sync_timeout() -> u64 {
    30000
}

fn default_initial_sync_limit() -> u64 {
    10
}

/// Room-level policy configuration for Matrix channels.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixRoomPolicy {
    /// Only process messages that @mention the bot
    pub require_mention: Option<bool>,
    /// Allow messages from bots
    pub allow_bots: Option<bool>,
    /// Allowed user IDs in this room (empty = allow all)
    pub users: Option<Vec<String>>,
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

    /// Enable mention gating - only process messages that @mention the bot
    #[serde(default)]
    pub mention_gating: bool,

    /// Allowed user IDs (empty = allow all users)
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Sync state persistent store path (default: ~/.aleph/state/matrix_sync.db)
    #[serde(default)]
    pub state_store_path: Option<String>,

    /// Event deduplication store path (default: ~/.aleph/state/matrix_dedupe.db)
    #[serde(default)]
    pub dedupe_store_path: Option<String>,

    /// Initial sync history message limit
    #[serde(default = "default_initial_sync_limit")]
    pub initial_sync_limit: u64,

    /// Automatically join rooms when invited
    #[serde(default)]
    pub auto_join_invites: bool,

    /// Room-level policy overrides
    #[serde(default)]
    pub rooms: HashMap<String, MatrixRoomPolicy>,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            homeserver_url: String::new(),
            access_token: String::new(),
            allowed_rooms: Vec::new(),
            sync_timeout_ms: 30000,
            send_typing: true,
            mention_gating: false,
            allowed_users: Vec::new(),
            state_store_path: None,
            dedupe_store_path: None,
            initial_sync_limit: 10,
            auto_join_invites: false,
            rooms: HashMap::new(),
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
            return Err("homeserver_url must start with 'http://' or 'https://'".to_string());
        }
        if self.access_token.is_empty() {
            return Err("access_token is required".to_string());
        }
        Ok(())
    }

    /// Return the default state store path.
    pub fn default_state_store_path() -> String {
        dirs::data_dir()
            .map(|p| p.join("aleph").join("state").join("matrix_sync.db"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "./matrix_sync.db".to_string())
    }

    /// Return the default dedupe store path.
    pub fn default_dedupe_store_path() -> String {
        dirs::data_dir()
            .map(|p| p.join("aleph").join("state").join("matrix_dedupe.db"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "./matrix_dedupe.db".to_string())
    }

    /// Resolve state store path.
    pub fn state_store_path(&self) -> String {
        self.state_store_path
            .clone()
            .unwrap_or_else(Self::default_state_store_path)
    }

    /// Resolve dedupe store path.
    pub fn dedupe_store_path(&self) -> String {
        self.dedupe_store_path
            .clone()
            .unwrap_or_else(Self::default_dedupe_store_path)
    }

    /// Check if a room ID is allowed
    pub fn is_room_allowed(&self, room_id: &str) -> bool {
        if self.allowed_rooms.is_empty() {
            true
        } else {
            self.allowed_rooms.contains(&room_id.to_string())
        }
    }

    /// Check if a user ID is allowed
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.iter().any(|u| u == user_id)
        }
    }

    /// Get effective mention gating for a room.
    pub fn is_mention_gating_enabled(&self, _room_id: &str) -> bool {
        self.mention_gating
    }

    /// Check if a user is allowed in a specific room.
    pub fn is_user_allowed_in_room(&self, user_id: &str, room_id: &str) -> bool {
        let base_allowed = self.is_user_allowed(user_id);
        if !base_allowed {
            return false;
        }
        if let Some(policy) = self.rooms.get(room_id) {
            if let Some(ref users) = policy.users {
                if !users.is_empty() && !users.iter().any(|u| u == user_id) {
                    return false;
                }
            }
        }
        true
    }

    pub fn check_mention(&self, body: &str, user_id: &str) -> bool {
        if !self.mention_gating {
            return true;
        }

        // Check for exact user_id as a Matrix mention (starts with @, contains :)
        // Pattern: @userid:server - the mention ends at : or whitespace/punctuation
        let stripped = user_id.trim_start_matches('@');
        let mention_pattern = format!("@{}", stripped);
        body.split_whitespace().any(|word| {
            word == stripped
                || word == mention_pattern
                || word.starts_with(&format!("@{}:", stripped))
                || word.starts_with(&format!("{}:", mention_pattern))
        })
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
        assert_eq!(config.initial_sync_limit, 10);
        assert!(!config.auto_join_invites);
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
        assert_eq!(config.validate().unwrap_err(), "access_token is required");
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
            mention_gating: true,
            allowed_users: vec!["@alice:matrix.org".to_string()],
            state_store_path: Some("/tmp/matrix_sync.db".to_string()),
            dedupe_store_path: Some("/tmp/matrix_dedupe.db".to_string()),
            initial_sync_limit: 50,
            auto_join_invites: true,
            rooms: {
                let mut m = HashMap::new();
                m.insert(
                    "!room1:matrix.org".to_string(),
                    MatrixRoomPolicy {
                        require_mention: Some(true),
                        allow_bots: Some(false),
                        users: Some(vec!["@alice:matrix.org".to_string()]),
                    },
                );
                m
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MatrixConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.homeserver_url, config.homeserver_url);
        assert_eq!(deserialized.access_token, config.access_token);
        assert_eq!(deserialized.allowed_rooms, config.allowed_rooms);
        assert_eq!(deserialized.sync_timeout_ms, config.sync_timeout_ms);
        assert_eq!(deserialized.send_typing, config.send_typing);
        assert_eq!(deserialized.mention_gating, config.mention_gating);
        assert_eq!(deserialized.allowed_users, config.allowed_users);
        assert_eq!(deserialized.state_store_path, config.state_store_path);
        assert_eq!(deserialized.dedupe_store_path, config.dedupe_store_path);
        assert_eq!(deserialized.initial_sync_limit, config.initial_sync_limit);
        assert_eq!(deserialized.auto_join_invites, config.auto_join_invites);
        assert_eq!(deserialized.rooms.len(), config.rooms.len());
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"homeserver_url": "https://matrix.org", "access_token": "token123"}"#;
        let config: MatrixConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.sync_timeout_ms, 30000);
        assert!(config.send_typing);
        assert!(config.allowed_rooms.is_empty());
        assert!(!config.mention_gating);
        assert!(config.allowed_users.is_empty());
        assert_eq!(config.initial_sync_limit, 10);
        assert!(!config.auto_join_invites);
    }

    #[test]
    fn test_room_policy_user_allowlist() {
        let mut config = MatrixConfig::default();
        config.rooms.insert(
            "!room1:matrix.org".to_string(),
            MatrixRoomPolicy {
                require_mention: None,
                allow_bots: None,
                users: Some(vec!["@alice:matrix.org".to_string()]),
            },
        );

        assert!(config.is_user_allowed_in_room("@alice:matrix.org", "!room1:matrix.org"));
        assert!(!config.is_user_allowed_in_room("@bob:matrix.org", "!room1:matrix.org"));
        // Other rooms fall back to global allowlist
        assert!(config.is_user_allowed_in_room("@bob:matrix.org", "!room2:matrix.org"));
    }
}

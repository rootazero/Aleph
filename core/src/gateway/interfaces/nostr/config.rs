//! Nostr Channel Configuration
//!
//! Configuration types for the Nostr relay integration using NIP-01 WebSocket protocol.
//! Supports connecting to multiple relays and filtering by public key.

use serde::{Deserialize, Serialize};

/// Default subscription kinds: text notes (1) and encrypted DMs (4).
fn default_subscription_kinds() -> Vec<u64> {
    vec![1, 4]
}

/// Nostr channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrConfig {
    /// Private key (hex-encoded 32 bytes)
    ///
    /// Used to derive the public key and sign events.
    /// Must be a valid 64-character hex string representing 32 bytes.
    pub private_key: String,

    /// Relay URLs to connect to (wss://... or ws://...)
    ///
    /// At least one relay is required. The channel connects to the first relay
    /// for real-time events and publishes to all relays.
    pub relays: Vec<String>,

    /// Allowed public keys to accept messages from (hex-encoded, empty = accept all)
    ///
    /// When non-empty, only events from these public keys are forwarded as
    /// inbound messages. Other events are silently dropped.
    #[serde(default)]
    pub allowed_pubkeys: Vec<String>,

    /// Event kinds to subscribe to (default: [1, 4] for text notes and DMs)
    ///
    /// - Kind 1: Text note (public)
    /// - Kind 4: Encrypted direct message (NIP-04, plaintext for now)
    #[serde(default = "default_subscription_kinds")]
    pub subscription_kinds: Vec<u64>,
}

impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            private_key: String::new(),
            relays: Vec::new(),
            allowed_pubkeys: Vec::new(),
            subscription_kinds: default_subscription_kinds(),
        }
    }
}

impl NostrConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.private_key.is_empty() {
            return Err("private_key is required".to_string());
        }

        // Validate hex format (64 hex chars = 32 bytes)
        if self.private_key.len() != 64 {
            return Err(format!(
                "private_key must be 64 hex characters (32 bytes), got {} chars",
                self.private_key.len()
            ));
        }
        if hex::decode(&self.private_key).is_err() {
            return Err("private_key must be valid hex".to_string());
        }

        if self.relays.is_empty() {
            return Err("at least one relay URL is required".to_string());
        }

        for relay in &self.relays {
            if !relay.starts_with("ws://") && !relay.starts_with("wss://") {
                return Err(format!(
                    "relay URL must start with 'ws://' or 'wss://': {relay}"
                ));
            }
        }

        // Validate allowed pubkeys format
        for pk in &self.allowed_pubkeys {
            if pk.len() != 64 {
                return Err(format!(
                    "allowed_pubkeys must be 64 hex characters each, got {} chars",
                    pk.len()
                ));
            }
            if hex::decode(pk).is_err() {
                return Err(format!("invalid hex in allowed_pubkeys: {pk}"));
            }
        }

        Ok(())
    }

    /// Check if a public key is allowed
    ///
    /// Returns true if allowed_pubkeys is empty (allow all) or if the given
    /// pubkey is in the allowed list.
    pub fn is_pubkey_allowed(&self, pubkey: &str) -> bool {
        if self.allowed_pubkeys.is_empty() {
            true
        } else {
            self.allowed_pubkeys.iter().any(|pk| pk == pubkey)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A valid 32-byte hex private key for testing (NOT a real key)
    const TEST_PRIVKEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TEST_PUBKEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn test_default_config() {
        let config = NostrConfig::default();
        assert!(config.private_key.is_empty());
        assert!(config.relays.is_empty());
        assert!(config.allowed_pubkeys.is_empty());
        assert_eq!(config.subscription_kinds, vec![1, 4]);
    }

    #[test]
    fn test_validate_empty_private_key() {
        let config = NostrConfig::default();
        let err = config.validate().unwrap_err();
        assert_eq!(err, "private_key is required");
    }

    #[test]
    fn test_validate_short_private_key() {
        let config = NostrConfig {
            private_key: "abcdef".to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("64 hex characters"), "Error: {err}");
    }

    #[test]
    fn test_validate_invalid_hex_private_key() {
        let config = NostrConfig {
            private_key: "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
                .to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("valid hex"), "Error: {err}");
    }

    #[test]
    fn test_validate_no_relays() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: Vec::new(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "at least one relay URL is required");
    }

    #[test]
    fn test_validate_invalid_relay_url() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["https://relay.example.com".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("ws://") || err.contains("wss://"), "Error: {err}");
    }

    #[test]
    fn test_validate_valid_config_wss() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["wss://relay.damus.io".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_config_ws() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["ws://localhost:7777".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_multiple_relays() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec![
                "wss://relay.damus.io".to_string(),
                "wss://nos.lol".to_string(),
                "wss://relay.nostr.band".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_allowed_pubkey_length() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            allowed_pubkeys: vec!["short".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("64 hex characters"), "Error: {err}");
    }

    #[test]
    fn test_validate_invalid_hex_allowed_pubkey() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            allowed_pubkeys: vec![
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
            ],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid hex"), "Error: {err}");
    }

    #[test]
    fn test_pubkey_allowed_empty_list() {
        let config = NostrConfig::default();
        assert!(config.is_pubkey_allowed(TEST_PUBKEY));
        assert!(config.is_pubkey_allowed("anything"));
    }

    #[test]
    fn test_pubkey_allowed_with_list() {
        let config = NostrConfig {
            allowed_pubkeys: vec![TEST_PUBKEY.to_string()],
            ..Default::default()
        };
        assert!(config.is_pubkey_allowed(TEST_PUBKEY));
        assert!(!config.is_pubkey_allowed(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["wss://relay.damus.io".to_string()],
            allowed_pubkeys: vec![TEST_PUBKEY.to_string()],
            subscription_kinds: vec![1, 4, 7],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: NostrConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.private_key, config.private_key);
        assert_eq!(deserialized.relays, config.relays);
        assert_eq!(deserialized.allowed_pubkeys, config.allowed_pubkeys);
        assert_eq!(deserialized.subscription_kinds, config.subscription_kinds);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{"private_key": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "relays": ["wss://relay.example.com"]}"#;
        let config: NostrConfig = serde_json::from_str(json).unwrap();

        assert!(config.allowed_pubkeys.is_empty());
        assert_eq!(config.subscription_kinds, vec![1, 4]);
    }
}

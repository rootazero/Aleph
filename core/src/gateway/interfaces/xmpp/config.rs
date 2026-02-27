//! XMPP Channel Configuration
//!
//! Configuration types for the XMPP channel with MUC (Multi-User Chat) support.
//! Uses raw TCP with manual XML stanza handling (RFC 6120/6121).

use serde::{Deserialize, Serialize};

fn default_port() -> u16 {
    5222
}

fn default_true() -> bool {
    true
}

fn default_nick() -> String {
    "aleph".to_string()
}

/// XMPP channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmppConfig {
    /// Full JID (e.g., "bot@example.com")
    pub jid: String,

    /// Password for SASL authentication
    pub password: String,

    /// XMPP server hostname (derived from JID if not set)
    #[serde(default)]
    pub server: Option<String>,

    /// XMPP port (default: 5222)
    #[serde(default = "default_port")]
    pub port: u16,

    /// MUC rooms to join (e.g., ["room@conference.example.com"])
    #[serde(default)]
    pub muc_rooms: Vec<String>,

    /// Whether to use TLS (default: true, reserved for future support)
    #[serde(default = "default_true")]
    pub use_tls: bool,

    /// Nickname used in MUC rooms (default: "aleph")
    #[serde(default = "default_nick")]
    pub nick: String,
}

impl Default for XmppConfig {
    fn default() -> Self {
        Self {
            jid: String::new(),
            password: String::new(),
            server: None,
            port: 5222,
            muc_rooms: Vec::new(),
            use_tls: true,
            nick: "aleph".to_string(),
        }
    }
}

impl XmppConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if !self.jid.contains('@') {
            return Err("jid must contain '@' (e.g., bot@example.com)".to_string());
        }
        if self.jid.is_empty() {
            return Err("jid is required".to_string());
        }
        if self.password.is_empty() {
            return Err("password is required".to_string());
        }
        for room in &self.muc_rooms {
            if !room.contains('@') {
                return Err(format!(
                    "MUC room '{}' must contain '@' (e.g., room@conference.example.com)",
                    room
                ));
            }
        }
        Ok(())
    }

    /// Extract the server host from the JID domain or use the explicit server setting.
    ///
    /// For JID "bot@example.com", returns "example.com" (unless `server` is explicitly set).
    pub fn server_host(&self) -> &str {
        if let Some(ref server) = self.server {
            server.as_str()
        } else {
            // Extract domain from JID: "user@domain/resource" -> "domain"
            self.jid
                .split('@')
                .nth(1)
                .and_then(|domain_part| {
                    // Strip resource if present: "domain/resource" -> "domain"
                    domain_part.split('/').next()
                })
                .unwrap_or("")
        }
    }

    /// Format the server address as `host:port`
    pub fn addr(&self) -> String {
        format!("{}:{}", self.server_host(), self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = XmppConfig::default();
        assert!(config.jid.is_empty());
        assert!(config.password.is_empty());
        assert!(config.server.is_none());
        assert_eq!(config.port, 5222);
        assert!(config.muc_rooms.is_empty());
        assert!(config.use_tls);
        assert_eq!(config.nick, "aleph");
    }

    #[test]
    fn test_validate_missing_at_in_jid() {
        let config = XmppConfig {
            jid: "botexample.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("jid must contain '@'"));
    }

    #[test]
    fn test_validate_empty_jid() {
        let config = XmppConfig {
            jid: String::new(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("jid must contain '@'"));
    }

    #[test]
    fn test_validate_empty_password() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: String::new(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("password is required"));
    }

    #[test]
    fn test_validate_invalid_muc_room() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            muc_rooms: vec!["room-no-domain".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("MUC room"));
        assert!(err.contains("must contain '@'"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            muc_rooms: vec!["room@conference.example.com".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_no_muc_rooms() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_host_from_jid() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            ..Default::default()
        };
        assert_eq!(config.server_host(), "example.com");
    }

    #[test]
    fn test_server_host_from_jid_with_resource() {
        let config = XmppConfig {
            jid: "bot@example.com/resource".to_string(),
            ..Default::default()
        };
        assert_eq!(config.server_host(), "example.com");
    }

    #[test]
    fn test_server_host_explicit() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            server: Some("xmpp.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(config.server_host(), "xmpp.example.com");
    }

    #[test]
    fn test_addr() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            port: 5222,
            ..Default::default()
        };
        assert_eq!(config.addr(), "example.com:5222");
    }

    #[test]
    fn test_addr_custom_port() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            port: 5223,
            server: Some("xmpp.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(config.addr(), "xmpp.example.com:5223");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret123".to_string(),
            server: Some("xmpp.example.com".to_string()),
            port: 5223,
            muc_rooms: vec![
                "room1@conference.example.com".to_string(),
                "room2@conference.example.com".to_string(),
            ],
            use_tls: false,
            nick: "mybot".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: XmppConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.jid, config.jid);
        assert_eq!(deserialized.password, config.password);
        assert_eq!(deserialized.server, config.server);
        assert_eq!(deserialized.port, config.port);
        assert_eq!(deserialized.muc_rooms, config.muc_rooms);
        assert_eq!(deserialized.use_tls, config.use_tls);
        assert_eq!(deserialized.nick, config.nick);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r##"{"jid": "bot@example.com", "password": "secret"}"##;
        let config: XmppConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.port, 5222);
        assert!(config.server.is_none());
        assert!(config.muc_rooms.is_empty());
        assert!(config.use_tls);
        assert_eq!(config.nick, "aleph");
    }

    #[test]
    fn test_serde_minimal() {
        let json = r##"{"jid": "bot@example.com", "password": "secret"}"##;
        let config: XmppConfig = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_serde_with_null_server() {
        let json = r##"{"jid": "bot@example.com", "password": "secret", "server": null}"##;
        let config: XmppConfig = serde_json::from_str(json).unwrap();
        assert!(config.server.is_none());
        assert_eq!(config.server_host(), "example.com");
    }

    #[test]
    fn test_validate_multiple_muc_rooms() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            muc_rooms: vec![
                "room1@conference.example.com".to_string(),
                "room2@conference.example.com".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_mixed_muc_rooms_invalid() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            muc_rooms: vec![
                "room1@conference.example.com".to_string(),
                "invalid-room".to_string(),
            ],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("invalid-room"));
    }
}

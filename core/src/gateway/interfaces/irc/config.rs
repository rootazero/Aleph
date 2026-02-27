//! IRC Channel Configuration
//!
//! Configuration types for the IRC channel using raw TCP (RFC 2812).

use serde::{Deserialize, Serialize};

fn default_port() -> u16 {
    6667
}

fn default_realname() -> String {
    "Aleph Bot".to_string()
}

/// IRC channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrcConfig {
    /// IRC server hostname (e.g., "irc.libera.chat")
    pub server: String,

    /// IRC server port (default: 6667 for plaintext, 6697 for TLS)
    #[serde(default = "default_port")]
    pub port: u16,

    /// Bot's IRC nickname
    pub nick: String,

    /// NickServ password (optional, for registered nicks)
    #[serde(default)]
    pub password: Option<String>,

    /// IRC channels to join (e.g., ["#aleph", "#test"])
    pub channels: Vec<String>,

    /// Use TLS (reserved for future support)
    #[serde(default)]
    pub use_tls: bool,

    /// Real name shown in WHOIS (default: "Aleph Bot")
    #[serde(default = "default_realname")]
    pub realname: String,
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            port: 6667,
            nick: String::new(),
            password: None,
            channels: Vec::new(),
            use_tls: false,
            realname: "Aleph Bot".to_string(),
        }
    }
}

impl IrcConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.server.is_empty() {
            return Err("server is required".to_string());
        }
        if self.nick.is_empty() {
            return Err("nick is required".to_string());
        }
        if self.channels.is_empty() {
            return Err("at least one channel is required".to_string());
        }
        for ch in &self.channels {
            if !ch.starts_with('#') && !ch.starts_with('&') {
                return Err(format!(
                    "channel '{}' must start with '#' or '&'",
                    ch
                ));
            }
        }
        Ok(())
    }

    /// Format the server address as `host:port`
    pub fn addr(&self) -> String {
        format!("{}:{}", self.server, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IrcConfig::default();
        assert!(config.server.is_empty());
        assert!(config.nick.is_empty());
        assert_eq!(config.port, 6667);
        assert!(config.password.is_none());
        assert!(config.channels.is_empty());
        assert!(!config.use_tls);
        assert_eq!(config.realname, "Aleph Bot");
    }

    #[test]
    fn test_validate_empty_server() {
        let config = IrcConfig {
            nick: "bot".to_string(),
            channels: vec!["#test".to_string()],
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), "server is required");
    }

    #[test]
    fn test_validate_empty_nick() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            channels: vec!["#test".to_string()],
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), "nick is required");
    }

    #[test]
    fn test_validate_no_channels() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "bot".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            "at least one channel is required"
        );
    }

    #[test]
    fn test_validate_invalid_channel_name() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "bot".to_string(),
            channels: vec!["nochanprefix".to_string()],
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("must start with '#' or '&'"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "alephbot".to_string(),
            channels: vec!["#aleph".to_string(), "&local".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_with_password() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "alephbot".to_string(),
            password: Some("secret".to_string()),
            channels: vec!["#test".to_string()],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_addr() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            port: 6667,
            ..Default::default()
        };
        assert_eq!(config.addr(), "irc.libera.chat:6667");
    }

    #[test]
    fn test_addr_custom_port() {
        let config = IrcConfig {
            server: "localhost".to_string(),
            port: 6697,
            ..Default::default()
        };
        assert_eq!(config.addr(), "localhost:6697");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            port: 6697,
            nick: "alephbot".to_string(),
            password: Some("secret123".to_string()),
            channels: vec!["#aleph".to_string(), "#test".to_string()],
            use_tls: true,
            realname: "Aleph IRC Bot".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: IrcConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.server, config.server);
        assert_eq!(deserialized.port, config.port);
        assert_eq!(deserialized.nick, config.nick);
        assert_eq!(deserialized.password, config.password);
        assert_eq!(deserialized.channels, config.channels);
        assert_eq!(deserialized.use_tls, config.use_tls);
        assert_eq!(deserialized.realname, config.realname);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r##"{"server": "irc.libera.chat", "nick": "bot", "channels": ["#test"]}"##;
        let config: IrcConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.port, 6667);
        assert!(config.password.is_none());
        assert!(!config.use_tls);
        assert_eq!(config.realname, "Aleph Bot");
    }

    #[test]
    fn test_serde_minimal() {
        // Minimum valid JSON
        let json = r##"{"server": "irc.libera.chat", "nick": "bot", "channels": ["#test"]}"##;
        let config: IrcConfig = serde_json::from_str(json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_serde_null_password() {
        let json = r##"{"server": "irc.libera.chat", "nick": "bot", "channels": ["#test"], "password": null}"##;
        let config: IrcConfig = serde_json::from_str(json).unwrap();
        assert!(config.password.is_none());
    }
}

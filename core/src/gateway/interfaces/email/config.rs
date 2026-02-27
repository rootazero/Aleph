//! Email Channel Configuration
//!
//! Configuration types for the Email channel using IMAP for receiving
//! and SMTP for sending messages.

use serde::{Deserialize, Serialize};

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    587
}

fn default_poll_interval() -> u64 {
    30
}

fn default_folders() -> Vec<String> {
    vec!["INBOX".to_string()]
}

fn default_true() -> bool {
    true
}

/// Email channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// IMAP server host (e.g., "imap.gmail.com")
    pub imap_host: String,

    /// IMAP port (default: 993 for IMAPS)
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,

    /// SMTP server host (e.g., "smtp.gmail.com")
    pub smtp_host: String,

    /// SMTP port (default: 587 for STARTTLS)
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,

    /// Login username (usually the email address)
    pub username: String,

    /// Login password (will use Zeroizing at runtime)
    pub password: String,

    /// From address for outgoing emails (e.g., "aleph@example.com")
    pub from_address: String,

    /// Poll interval in seconds (default: 30)
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// IMAP folders to monitor (default: ["INBOX"])
    #[serde(default = "default_folders")]
    pub folders: Vec<String>,

    /// Only process emails from these senders (empty = allow all)
    #[serde(default)]
    pub allowed_senders: Vec<String>,

    /// Use TLS for connections (default: true)
    #[serde(default = "default_true")]
    pub use_tls: bool,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 587,
            username: String::new(),
            password: String::new(),
            from_address: String::new(),
            poll_interval_secs: 30,
            folders: vec!["INBOX".to_string()],
            allowed_senders: Vec::new(),
            use_tls: true,
        }
    }
}

impl EmailConfig {
    /// Create config from environment variables
    pub fn from_env() -> Option<Self> {
        let imap_host = std::env::var("EMAIL_IMAP_HOST").ok()?;
        let smtp_host = std::env::var("EMAIL_SMTP_HOST").ok()?;
        let username = std::env::var("EMAIL_USERNAME").ok()?;
        let password = std::env::var("EMAIL_PASSWORD").ok()?;
        let from_address = std::env::var("EMAIL_FROM_ADDRESS")
            .unwrap_or_else(|_| username.clone());

        Some(Self {
            imap_host,
            smtp_host,
            username,
            password,
            from_address,
            ..Default::default()
        })
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.imap_host.is_empty() {
            return Err("imap_host is required".to_string());
        }
        if self.smtp_host.is_empty() {
            return Err("smtp_host is required".to_string());
        }
        if self.username.is_empty() {
            return Err("username is required".to_string());
        }
        if self.password.is_empty() {
            return Err("password is required".to_string());
        }
        if self.from_address.is_empty() {
            return Err("from_address is required".to_string());
        }
        if !self.from_address.contains('@') {
            return Err("from_address must be a valid email address".to_string());
        }
        if self.poll_interval_secs == 0 {
            return Err("poll_interval_secs must be > 0".to_string());
        }
        if self.folders.is_empty() {
            return Err("at least one folder must be specified".to_string());
        }
        Ok(())
    }

    /// Check if a sender email is allowed
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.allowed_senders.is_empty() {
            true
        } else {
            self.allowed_senders
                .iter()
                .any(|allowed| sender.contains(allowed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmailConfig::default();
        assert!(config.imap_host.is_empty());
        assert_eq!(config.imap_port, 993);
        assert!(config.smtp_host.is_empty());
        assert_eq!(config.smtp_port, 587);
        assert!(config.username.is_empty());
        assert!(config.password.is_empty());
        assert!(config.from_address.is_empty());
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.folders, vec!["INBOX"]);
        assert!(config.allowed_senders.is_empty());
        assert!(config.use_tls);
    }

    #[test]
    fn test_validate_empty_imap_host() {
        let config = EmailConfig::default();
        let err = config.validate().unwrap_err();
        assert_eq!(err, "imap_host is required");
    }

    #[test]
    fn test_validate_empty_smtp_host() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "smtp_host is required");
    }

    #[test]
    fn test_validate_empty_username() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "username is required");
    }

    #[test]
    fn test_validate_empty_password() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            username: "user@example.com".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "password is required");
    }

    #[test]
    fn test_validate_empty_from_address() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            username: "user@example.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err, "from_address is required");
    }

    #[test]
    fn test_validate_invalid_from_address() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            username: "user@example.com".to_string(),
            password: "secret".to_string(),
            from_address: "not-an-email".to_string(),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("valid email address"));
    }

    #[test]
    fn test_validate_zero_poll_interval() {
        let config = EmailConfig {
            imap_host: "imap.example.com".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            username: "user@example.com".to_string(),
            password: "secret".to_string(),
            from_address: "aleph@example.com".to_string(),
            poll_interval_secs: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("poll_interval_secs"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = EmailConfig {
            imap_host: "imap.gmail.com".to_string(),
            smtp_host: "smtp.gmail.com".to_string(),
            username: "user@gmail.com".to_string(),
            password: "app-password".to_string(),
            from_address: "aleph@gmail.com".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sender_allowed_empty_list() {
        let config = EmailConfig::default();
        assert!(config.is_sender_allowed("anyone@anywhere.com"));
        assert!(config.is_sender_allowed("user@example.com"));
    }

    #[test]
    fn test_sender_allowed_with_list() {
        let config = EmailConfig {
            allowed_senders: vec![
                "boss@company.com".to_string(),
                "admin@company.com".to_string(),
            ],
            ..Default::default()
        };
        assert!(config.is_sender_allowed("boss@company.com"));
        assert!(config.is_sender_allowed("admin@company.com"));
        assert!(!config.is_sender_allowed("random@other.com"));
    }

    #[test]
    fn test_sender_allowed_partial_match() {
        let config = EmailConfig {
            allowed_senders: vec!["@company.com".to_string()],
            ..Default::default()
        };
        assert!(config.is_sender_allowed("boss@company.com"));
        assert!(config.is_sender_allowed("intern@company.com"));
        assert!(!config.is_sender_allowed("user@other.com"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = EmailConfig {
            imap_host: "imap.gmail.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".to_string(),
            smtp_port: 465,
            username: "user@gmail.com".to_string(),
            password: "secret123".to_string(),
            from_address: "aleph@gmail.com".to_string(),
            poll_interval_secs: 60,
            folders: vec!["INBOX".to_string(), "Important".to_string()],
            allowed_senders: vec!["boss@work.com".to_string()],
            use_tls: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EmailConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.imap_host, config.imap_host);
        assert_eq!(deserialized.imap_port, config.imap_port);
        assert_eq!(deserialized.smtp_host, config.smtp_host);
        assert_eq!(deserialized.smtp_port, config.smtp_port);
        assert_eq!(deserialized.username, config.username);
        assert_eq!(deserialized.password, config.password);
        assert_eq!(deserialized.from_address, config.from_address);
        assert_eq!(deserialized.poll_interval_secs, config.poll_interval_secs);
        assert_eq!(deserialized.folders, config.folders);
        assert_eq!(deserialized.allowed_senders, config.allowed_senders);
        assert_eq!(deserialized.use_tls, config.use_tls);
    }

    #[test]
    fn test_serde_defaults() {
        let json = r#"{
            "imap_host": "imap.test.com",
            "smtp_host": "smtp.test.com",
            "username": "user@test.com",
            "password": "pass",
            "from_address": "aleph@test.com"
        }"#;
        let config: EmailConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.imap_port, 993);
        assert_eq!(config.smtp_port, 587);
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.folders, vec!["INBOX"]);
        assert!(config.allowed_senders.is_empty());
        assert!(config.use_tls);
    }
}

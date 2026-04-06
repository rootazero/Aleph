use crate::gateway::channel::ChannelError;
use serde::Deserialize;

fn default_domain() -> String {
    "feishu".to_string()
}
fn default_true() -> bool {
    true
}
fn default_render_mode() -> String {
    "auto".to_string()
}
fn default_webhook_port() -> u16 {
    3000
}
fn default_webhook_host() -> String {
    "127.0.0.1".to_string()
}
fn default_webhook_path() -> String {
    "/feishu/events".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    pub bot_name: Option<String>,
    #[serde(default = "default_true")]
    pub dm_allowed: bool,
    #[serde(default)]
    pub groups_allowed: bool,
    #[serde(default = "default_group_policy")]
    pub group_policy: String,
    #[serde(default)]
    pub group_allowlist: Vec<String>,
    #[serde(default = "default_true")]
    pub require_mention: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
    #[serde(default = "default_true")]
    pub reaction_notifications: bool,
    #[serde(default)]
    pub group_session_scope: GroupSessionScope,
    /// Connection mode: "websocket" (default) or "webhook"
    #[serde(default = "default_connection_mode")]
    pub connection_mode: String,
    /// Webhook server port (only used when connection_mode = "webhook")
    #[serde(default = "default_webhook_port")]
    pub webhook_port: u16,
    /// Webhook server host (only used when connection_mode = "webhook")
    #[serde(default = "default_webhook_host")]
    pub webhook_host: String,
    /// Webhook endpoint path (only used when connection_mode = "webhook")
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,
    /// Verification token for webhook URL verification (only used when connection_mode = "webhook")
    pub verification_token: Option<String>,
    /// Encrypt key for webhook signature verification (only used when connection_mode = "webhook")
    pub encrypt_key: Option<String>,
    #[serde(default)]
    pub accounts: Option<std::collections::HashMap<String, FeishuAccountEntry>>,
    #[serde(default)]
    pub default_account: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FeishuAccountEntry {
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub enabled: Option<bool>,
}

fn default_connection_mode() -> String {
    "websocket".to_string()
}

fn default_group_policy() -> String {
    "open".to_string()
}

/// How group conversations are scoped for session management.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSessionScope {
    /// One session per group chat (default)
    #[default]
    Group,
    /// One session per user within a group
    User,
    /// One session per message thread (root_id)
    Thread,
}

impl FeishuConfig {
    /// Resolve the base URL from the domain field.
    pub fn base_url(&self) -> String {
        match self.domain.as_str() {
            "feishu" => "https://open.feishu.cn".to_string(),
            "lark" => "https://open.larksuite.com".to_string(),
            custom => custom.trim_end_matches('/').to_string(),
        }
    }

    /// Returns true if webhook mode is enabled.
    pub fn is_webhook_mode(&self) -> bool {
        self.connection_mode == "webhook"
    }

    pub fn is_group_allowed(&self, chat_id: &str) -> bool {
        if !self.groups_allowed {
            return false;
        }
        match self.group_policy.as_str() {
            "allowlist" => self.group_allowlist.iter().any(|id| id == chat_id),
            _ => true,
        }
    }

    /// Validate the config and return an error if required fields are missing.
    pub fn validate(&self) -> Result<(), ChannelError> {
        if self.app_id.is_empty() {
            return Err(ChannelError::ConfigError("app_id is required".to_string()));
        }
        if self.app_secret.is_empty() {
            return Err(ChannelError::ConfigError(
                "app_secret is required".to_string(),
            ));
        }
        if self.is_webhook_mode() {
            if self.verification_token.is_none() {
                return Err(ChannelError::ConfigError(
                    "verification_token is required for webhook mode".to_string(),
                ));
            }
            if self.encrypt_key.is_none() {
                return Err(ChannelError::ConfigError(
                    "encrypt_key is required for webhook mode".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn resolve_default_account_id(&self) -> String {
        self.default_account
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    pub fn list_account_ids(&self) -> Vec<String> {
        self.accounts
            .as_ref()
            .map(|accounts| accounts.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn resolve_credentials(&self, account_id: &str) -> Option<(String, String)> {
        let target_id = if account_id.is_empty() || account_id == "default" {
            None
        } else {
            Some(account_id)
        };

        match target_id {
            None => {
                if self.app_id.is_empty() || self.app_secret.is_empty() {
                    None
                } else {
                    Some((self.app_id.clone(), self.app_secret.clone()))
                }
            }
            Some(id) => self.accounts.as_ref().and_then(|accounts| {
                accounts.get(id).and_then(|entry| {
                    let app_id = entry
                        .app_id
                        .as_ref()
                        .or(Some(&self.app_id))
                        .filter(|id| !id.is_empty())?;
                    let app_secret = entry
                        .app_secret
                        .as_ref()
                        .or(Some(&self.app_secret))
                        .filter(|s| !s.is_empty())?;
                    Some((app_id.clone(), app_secret.clone()))
                })
            }),
        }
    }

    pub fn is_account_enabled(&self, account_id: &str) -> bool {
        if account_id.is_empty() || account_id == "default" {
            return true;
        }
        self.accounts
            .as_ref()
            .and_then(|accounts| accounts.get(account_id))
            .and_then(|entry| entry.enabled)
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_feishu() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert_eq!(config.base_url(), "https://open.feishu.cn");
    }

    #[test]
    fn test_base_url_lark() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "lark".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert_eq!(config.base_url(), "https://open.larksuite.com");
    }

    #[test]
    fn test_base_url_custom() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "https://my.feishu.internal/".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert_eq!(config.base_url(), "https://my.feishu.internal");
    }

    #[test]
    fn test_config_deserialization_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "feishu");
        assert!(config.dm_allowed);
        assert!(!config.groups_allowed);
        assert!(config.require_mention);
        assert!(config.bot_name.is_none());
        assert_eq!(config.group_session_scope, GroupSessionScope::Group);
    }

    #[test]
    fn test_config_deserialization_full() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123",
            "domain": "lark",
            "bot_name": "MyBot",
            "dm_allowed": false,
            "groups_allowed": true,
            "require_mention": false,
            "group_session_scope": "thread"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "lark");
        assert!(!config.dm_allowed);
        assert!(config.groups_allowed);
        assert!(!config.require_mention);
        assert_eq!(config.bot_name.as_deref(), Some("MyBot"));
        assert_eq!(config.group_session_scope, GroupSessionScope::Thread);
    }

    #[test]
    fn test_config_streaming_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.streaming);
        assert_eq!(config.render_mode, "auto");
        assert!(config.typing_indicator);
    }

    #[test]
    fn test_config_streaming_overrides() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret",
            "streaming": false,
            "render_mode": "card",
            "typing_indicator": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(!config.streaming);
        assert_eq!(config.render_mode, "card");
        assert!(!config.typing_indicator);
    }

    #[test]
    fn test_validate_ok() {
        let config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_app_id() {
        let config = FeishuConfig {
            app_id: "".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_app_secret() {
        let config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_resolve_default_account_id_explicit() {
        let mut config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: Some("work".to_string()),
        };
        assert_eq!(config.resolve_default_account_id(), "work");

        config.default_account = None;
        assert_eq!(config.resolve_default_account_id(), "default");

        config.default_account = Some("".to_string());
        assert_eq!(config.resolve_default_account_id(), "default");
    }

    #[test]
    fn test_list_account_ids() {
        let config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: Some(std::collections::HashMap::from([
                (
                    "work".to_string(),
                    FeishuAccountEntry {
                        app_id: Some("cli_work".to_string()),
                        app_secret: Some("secret_work".to_string()),
                        enabled: Some(true),
                    },
                ),
                (
                    "personal".to_string(),
                    FeishuAccountEntry {
                        app_id: Some("cli_personal".to_string()),
                        app_secret: Some("secret_personal".to_string()),
                        enabled: Some(false),
                    },
                ),
            ])),
            default_account: Some("work".to_string()),
        };
        let ids = config.list_account_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"work".to_string()));
        assert!(ids.contains(&"personal".to_string()));
    }

    #[test]
    fn test_resolve_credentials_base() {
        let config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: None,
            default_account: None,
        };
        assert_eq!(
            config.resolve_credentials("default"),
            Some(("cli_xxx".to_string(), "secret".to_string()))
        );
        assert_eq!(
            config.resolve_credentials(""),
            Some(("cli_xxx".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn test_resolve_credentials_account_override() {
        let config = FeishuConfig {
            app_id: "cli_base".into(),
            app_secret: "secret_base".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: Some(std::collections::HashMap::from([(
                "work".to_string(),
                FeishuAccountEntry {
                    app_id: Some("cli_work".to_string()),
                    app_secret: Some("secret_work".to_string()),
                    enabled: Some(true),
                },
            )])),
            default_account: Some("work".to_string()),
        };
        assert_eq!(
            config.resolve_credentials("work"),
            Some(("cli_work".to_string(), "secret_work".to_string()))
        );
        assert_eq!(
            config.resolve_credentials("default"),
            Some(("cli_base".to_string(), "secret_base".to_string()))
        );
    }

    #[test]
    fn test_is_account_enabled() {
        let config = FeishuConfig {
            app_id: "cli_xxx".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            group_policy: "open".into(),
            group_allowlist: vec![],
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            reaction_notifications: true,
            group_session_scope: GroupSessionScope::default(),
            connection_mode: "websocket".into(),
            webhook_port: 3000,
            webhook_host: "127.0.0.1".into(),
            webhook_path: "/feishu/events".into(),
            verification_token: None,
            encrypt_key: None,
            accounts: Some(std::collections::HashMap::from([
                (
                    "enabled_acc".to_string(),
                    FeishuAccountEntry {
                        app_id: None,
                        app_secret: None,
                        enabled: Some(true),
                    },
                ),
                (
                    "disabled_acc".to_string(),
                    FeishuAccountEntry {
                        app_id: None,
                        app_secret: None,
                        enabled: Some(false),
                    },
                ),
            ])),
            default_account: None,
        };
        assert!(config.is_account_enabled("default"));
        assert!(config.is_account_enabled(""));
        assert!(config.is_account_enabled("enabled_acc"));
        assert!(!config.is_account_enabled("disabled_acc"));
    }
}

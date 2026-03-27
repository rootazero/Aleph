use serde::Deserialize;
use crate::gateway::channel::ChannelError;

fn default_domain() -> String { "feishu".to_string() }
fn default_true() -> bool { true }
fn default_render_mode() -> String { "auto".to_string() }

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
    #[serde(default = "default_true")]
    pub require_mention: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
    #[serde(default)]
    pub group_session_scope: GroupSessionScope,
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

    /// Validate the config and return an error if required fields are missing.
    pub fn validate(&self) -> Result<(), ChannelError> {
        if self.app_id.is_empty() {
            return Err(ChannelError::ConfigError("app_id is required".to_string()));
        }
        if self.app_secret.is_empty() {
            return Err(ChannelError::ConfigError("app_secret is required".to_string()));
        }
        Ok(())
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
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
            require_mention: true,
            streaming: true,
            render_mode: "auto".into(),
            typing_indicator: true,
            group_session_scope: GroupSessionScope::default(),
        };
        assert!(config.validate().is_err());
    }
}

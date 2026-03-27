use serde::Deserialize;

// ── Default helpers ──

fn default_domain() -> String { "feishu".to_string() }
fn default_true() -> bool { true }
fn default_render_mode() -> String { "auto".to_string() }

// ── Group Session Scope ──

/// Controls how group chat sessions are scoped for the bot.
///
/// - `Group`: one session per group chat (default)
/// - `GroupTopic`: one session per message thread (topic) within a group
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSessionScope {
    #[default]
    Group,
    GroupTopic,
}

// ── Config ──

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

impl FeishuConfig {
    /// Resolve the base URL from the domain field.
    pub fn base_url(&self) -> String {
        match self.domain.as_str() {
            "feishu" => "https://open.feishu.cn".to_string(),
            "lark" => "https://open.larksuite.com".to_string(),
            custom => custom.trim_end_matches('/').to_string(),
        }
    }

    /// Validate that required fields are present and values are within accepted ranges.
    ///
    /// Returns `Ok(())` if the config is valid, or `Err(String)` with a description of the problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.app_id.is_empty() {
            return Err("app_id must not be empty".to_string());
        }
        if self.app_secret.is_empty() {
            return Err("app_secret must not be empty".to_string());
        }
        if !matches!(self.render_mode.as_str(), "auto" | "card" | "raw") {
            return Err(format!(
                "render_mode must be 'auto', 'card', or 'raw'; got '{}'",
                self.render_mode
            ));
        }
        if let Some(name) = &self.bot_name {
            if name.is_empty() {
                return Err("bot_name must not be empty when set (omit the field instead)".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──

    fn minimal_config() -> FeishuConfig {
        FeishuConfig {
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
            group_session_scope: GroupSessionScope::Group,
        }
    }

    // ── base_url tests (migrated from types.rs) ──

    #[test]
    fn test_base_url_feishu() {
        let config = minimal_config();
        assert_eq!(config.base_url(), "https://open.feishu.cn");
    }

    #[test]
    fn test_base_url_lark() {
        let mut config = minimal_config();
        config.domain = "lark".into();
        assert_eq!(config.base_url(), "https://open.larksuite.com");
    }

    #[test]
    fn test_base_url_custom() {
        let mut config = minimal_config();
        config.domain = "https://my.feishu.internal/".into();
        assert_eq!(config.base_url(), "https://my.feishu.internal");
    }

    // ── deserialization tests (migrated from types.rs) ──

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
            "require_mention": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "lark");
        assert!(!config.dm_allowed);
        assert!(config.groups_allowed);
        assert!(!config.require_mention);
        assert_eq!(config.bot_name.as_deref(), Some("MyBot"));
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

    // ── GroupSessionScope tests ──

    #[test]
    fn test_group_session_scope_default() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.group_session_scope, GroupSessionScope::Group);
    }

    #[test]
    fn test_group_session_scope_group_topic() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret",
            "group_session_scope": "group_topic"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.group_session_scope, GroupSessionScope::GroupTopic);
    }

    #[test]
    fn test_group_session_scope_explicit_group() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret",
            "group_session_scope": "group"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.group_session_scope, GroupSessionScope::Group);
    }

    // ── validate() tests ──

    #[test]
    fn test_validate_ok() {
        let config = minimal_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_app_id() {
        let mut config = minimal_config();
        config.app_id = "".into();
        let err = config.validate().unwrap_err();
        assert!(err.contains("app_id"));
    }

    #[test]
    fn test_validate_empty_app_secret() {
        let mut config = minimal_config();
        config.app_secret = "".into();
        let err = config.validate().unwrap_err();
        assert!(err.contains("app_secret"));
    }

    #[test]
    fn test_validate_invalid_render_mode() {
        let mut config = minimal_config();
        config.render_mode = "markdown".into();
        let err = config.validate().unwrap_err();
        assert!(err.contains("render_mode"));
        assert!(err.contains("markdown"));
    }

    #[test]
    fn test_validate_render_mode_card() {
        let mut config = minimal_config();
        config.render_mode = "card".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_render_mode_raw() {
        let mut config = minimal_config();
        config.render_mode = "raw".into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_bot_name() {
        let mut config = minimal_config();
        config.bot_name = Some("".into());
        let err = config.validate().unwrap_err();
        assert!(err.contains("bot_name"));
    }

    #[test]
    fn test_validate_some_bot_name() {
        let mut config = minimal_config();
        config.bot_name = Some("Aleph".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_no_bot_name() {
        let mut config = minimal_config();
        config.bot_name = None;
        assert!(config.validate().is_ok());
    }
}

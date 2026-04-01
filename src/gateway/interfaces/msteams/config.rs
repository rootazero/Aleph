//! Microsoft Teams Channel Configuration

use serde::{Deserialize, Serialize};

/// Teams channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsTeamsConfig {
    /// Azure Bot App ID
    pub app_id: String,
    /// Azure Bot App Password (client secret)
    pub app_password: String,
    /// Azure AD Tenant ID (default: "common")
    #[serde(default = "default_tenant")]
    pub tenant_id: String,
    /// Allowed user AAD IDs (empty = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Allow group/team messages
    #[serde(default = "default_true")]
    pub groups_allowed: bool,
    /// Webhook path
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,
    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,
}

fn default_tenant() -> String {
    "common".into()
}
fn default_true() -> bool {
    true
}
fn default_webhook_path() -> String {
    "/msteams/messages".into()
}
impl Default for MsTeamsConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_password: String::new(),
            tenant_id: default_tenant(),
            allowed_users: Vec::new(),
            groups_allowed: true,
            webhook_path: default_webhook_path(),
            send_typing: true,
        }
    }
}

impl MsTeamsConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.app_id.is_empty() {
            return Err("app_id is required".into());
        }
        if self.app_password.is_empty() {
            return Err("app_password is required".into());
        }
        if !self.webhook_path.starts_with('/') {
            return Err("webhook_path must start with '/'".into());
        }
        Ok(())
    }

    pub fn is_user_allowed(&self, aad_id: &str) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.iter().any(|id| id == aad_id)
    }

    pub fn is_conversation_allowed(&self, conversation_type: Option<&str>) -> bool {
        match conversation_type {
            Some("groupChat") | Some("channel") => self.groups_allowed,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MsTeamsConfig::default();
        assert_eq!(config.tenant_id, "common");
        assert!(config.groups_allowed);
        assert_eq!(config.webhook_path, "/msteams/messages");
    }

    #[test]
    fn test_validate() {
        let mut config = MsTeamsConfig::default();
        assert!(config.validate().is_err());

        config.app_id = "app-id".into();
        assert!(config.validate().is_err());

        config.app_password = "secret".into();
        assert!(config.validate().is_ok());

        config.webhook_path = "no-slash".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_is_user_allowed() {
        let mut config = MsTeamsConfig::default();
        assert!(config.is_user_allowed("anyone"));

        config.allowed_users = vec!["user-1".into(), "user-2".into()];
        assert!(config.is_user_allowed("user-1"));
        assert!(!config.is_user_allowed("user-3"));
    }

    #[test]
    fn test_is_conversation_allowed() {
        let mut config = MsTeamsConfig::default();
        assert!(config.is_conversation_allowed(Some("personal")));
        assert!(config.is_conversation_allowed(Some("groupChat")));

        config.groups_allowed = false;
        assert!(config.is_conversation_allowed(Some("personal")));
        assert!(!config.is_conversation_allowed(Some("groupChat")));
        assert!(!config.is_conversation_allowed(Some("channel")));
    }

    #[test]
    fn test_deserialize_from_json() {
        let json = r#"{"app_id": "my-app", "app_password": "secret"}"#;
        let config: MsTeamsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.app_id, "my-app");
        assert_eq!(config.tenant_id, "common");
        assert!(config.groups_allowed);
    }
}

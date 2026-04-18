//! Microsoft Teams Channel Configuration

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::gateway::interfaces::msteams::auth::FederatedCredential;

/// Reply style for group messages.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyStyle {
    /// Reply in-thread to the user's message
    #[default]
    Thread,
    /// Reply as top-level message in the channel
    TopLevel,
}

/// Per-channel override settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelOverride {
    /// Require bot @mention to respond (default: true for group chats)
    #[serde(default)]
    pub require_mention: Option<bool>,
    /// Reply style override
    #[serde(default)]
    pub reply_style: Option<ReplyStyle>,
}

/// Per-team override settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamOverride {
    /// Per-channel overrides
    #[serde(default)]
    pub channels: HashMap<String, ChannelOverride>,
    /// Require bot @mention for this team (default: true for group chats)
    #[serde(default)]
    pub require_mention: Option<bool>,
    /// Reply style for this team
    #[serde(default)]
    pub reply_style: Option<ReplyStyle>,
}

/// Teams channel configuration
///
/// ## Azure AD App Registration
///
/// The app uses the Bot Framework credentials (`app_id` / `app_password`) for both
/// Bot Framework API calls and Microsoft Graph API calls via the `/.default` scope.
/// All Graph permissions must be pre-configured in the Azure AD app:
///
/// | Operation | Required Permission |
/// |---|---|
/// | `lookup_user` | `User.Read.All` or `People.Read` |
/// | `get_user` | `User.Read.All` |
/// | `lookup_team` | `Group.Read.All` |
/// | `list_team_channels` | `Channel.Read.All` |
/// | `get_message` / `list_messages` | `ChannelMessage.Read.All` |
/// | `download_media` / `upload_to_sharepoint` | `Files.ReadWrite.All` |
///
/// Add these under **API permissions** in the Azure portal (App registrations).
/// The `ChannelMessage.Read.All` permission requires admin consent.
///
/// ## Authentication Methods
///
/// Two authentication methods are supported:
/// 1. **Client Secret** (deprecated): `app_password` field
/// 2. **Federated Identity** (recommended): `federated_identity` field
///
/// Federated identity uses certificate-based auth or Azure Managed Identity.
///
/// ## Example Configuration
///
/// ```toml
/// [channels.msteams]
/// enabled = true
/// app_id = "YOUR-BOT-APP-ID"
/// app_password = "YOUR-CLIENT-SECRET"  # deprecated
/// tenant_id = "common"
/// groups_allowed = true
/// require_mention = false
/// reply_style = "thread"
///
/// [channels.msteams.federated_identity]  # recommended
/// certificate_path = "/path/to/cert.pem"
/// authority_url = "https://login.microsoftonline.com/{tenant}"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsTeamsConfig {
    /// Azure Bot App ID
    pub app_id: String,
    /// Azure Bot App Password (client secret) - deprecated, use federated_identity instead
    #[deprecated(since = "2026.06.01", note = "use federated_identity instead")]
    pub app_password: String,
    /// Federated identity configuration (recommended alternative to app_password)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub federated_identity: Option<FederatedCredential>,
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
    /// Require bot @mention in group chats to respond (default: true)
    #[serde(default = "default_true")]
    pub require_mention: bool,
    /// Default reply style for group messages
    #[serde(default)]
    pub reply_style: ReplyStyle,
    /// Per-team override settings (team_id -> config)
    #[serde(default)]
    pub teams: HashMap<String, TeamOverride>,
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
        #[allow(deprecated)]
        Self {
            app_id: String::new(),
            app_password: String::new(),
            federated_identity: None,
            tenant_id: default_tenant(),
            allowed_users: Vec::new(),
            groups_allowed: true,
            webhook_path: default_webhook_path(),
            send_typing: true,
            require_mention: true,
            reply_style: ReplyStyle::default(),
            teams: HashMap::new(),
        }
    }
}

impl MsTeamsConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.app_id.is_empty() {
            return Err("app_id is required".into());
        }
        #[allow(deprecated)]
        let app_password_empty = self.app_password.is_empty();
        if app_password_empty && self.federated_identity.is_none() {
            return Err("either app_password or federated_identity is required".into());
        }
        if let Some(ref fed) = self.federated_identity {
            if let Err(e) = fed.validate() {
                return Err(format!("federated_identity validation failed: {}", e));
            }
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

    /// Get effective require_mention for a conversation.
    ///
    /// Returns the per-team setting if available, otherwise the global setting.
    pub fn effective_require_mention(&self, team_id: Option<&str>) -> bool {
        if let Some(team_id) = team_id {
            if let Some(team) = self.teams.get(team_id) {
                if let Some(require_mention) = team.require_mention {
                    return require_mention;
                }
            }
        }
        self.require_mention
    }

    /// Get effective reply_style for a conversation.
    ///
    /// Returns the per-team setting if available, otherwise the global setting.
    pub fn effective_reply_style(&self, team_id: Option<&str>) -> ReplyStyle {
        if let Some(team_id) = team_id {
            if let Some(team) = self.teams.get(team_id) {
                if let Some(reply_style) = team.reply_style {
                    return reply_style;
                }
            }
        }
        self.reply_style
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

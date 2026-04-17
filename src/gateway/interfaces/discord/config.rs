//! Discord Channel Configuration
//!
//! Configuration types for the Discord Bot integration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// Legacy Types (for backward compatibility)
// ============================================================================

/// Discord channel configuration (legacy flat config)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Bot token from Discord Developer Portal
    pub bot_token: String,

    /// Application ID (for slash commands)
    #[serde(default)]
    pub application_id: Option<u64>,

    /// Allowed guild (server) IDs (empty = allow all)
    #[serde(default)]
    pub allowed_guilds: Vec<u64>,

    /// Allowed channel IDs within guilds (empty = allow all)
    #[serde(default)]
    pub allowed_channels: Vec<u64>,

    /// Allow direct messages
    #[serde(default = "default_true")]
    pub dm_allowed: bool,

    /// Prefix for text commands (e.g., "!")
    #[serde(default = "default_prefix")]
    pub command_prefix: String,

    /// Whether to respond to mentions
    #[serde(default = "default_true")]
    pub respond_to_mentions: bool,

    /// Whether to use slash commands
    #[serde(default = "default_true")]
    pub slash_commands_enabled: bool,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,

    /// Gateway intents to request
    #[serde(default)]
    pub intents: IntentsConfig,

    /// Thread binding configuration
    #[serde(default)]
    pub threads: ThreadConfig,
}

/// Thread binding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadConfig {
    /// Enable automatic thread binding
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Auto-create threads for new conversations
    #[serde(default)]
    pub auto_create: bool,

    /// Channel IDs where auto-threading is enabled (empty = all)
    #[serde(default)]
    pub auto_create_channels: Vec<u64>,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_create: false,
            auto_create_channels: Vec::new(),
        }
    }
}

/// Gateway intents configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentsConfig {
    /// Receive guild messages
    #[serde(default = "default_true")]
    pub guild_messages: bool,

    /// Receive direct messages
    #[serde(default = "default_true")]
    pub direct_messages: bool,

    /// Receive message content (requires privileged intent)
    #[serde(default = "default_true")]
    pub message_content: bool,

    /// Receive guild member events
    #[serde(default)]
    pub guild_members: bool,

    /// Receive guild thread events
    #[serde(default)]
    pub guild_threads: bool,
}

impl Default for IntentsConfig {
    fn default() -> Self {
        Self {
            guild_messages: true,
            direct_messages: true,
            message_content: true,
            guild_members: false,
            guild_threads: false,
        }
    }
}

// ============================================================================
// New Nested Config Types (for multi-account support)
// ============================================================================

/// Discord channel settings with per-guild/per-channel override support
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordChannelSettings {
    /// Command prefix (e.g., "/")
    #[serde(default)]
    pub prefix: Option<String>,

    /// Allowed channel IDs (empty = allow all)
    #[serde(default)]
    pub allowlist: Vec<u64>,

    /// Blocked channel IDs
    #[serde(default)]
    pub blocklist: Vec<u64>,

    /// Feature toggles
    #[serde(default)]
    pub features: DiscordFeatures,

    /// Thread binding configuration
    #[serde(default)]
    pub thread_binding: ThreadBindingConfig,

    /// Reply configuration
    #[serde(default)]
    pub reply: ReplyConfig,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
}

impl Default for DiscordChannelSettings {
    fn default() -> Self {
        Self {
            prefix: None,
            allowlist: Vec::new(),
            blocklist: Vec::new(),
            features: DiscordFeatures::default(),
            thread_binding: ThreadBindingConfig::default(),
            reply: ReplyConfig::default(),
            typing_indicator: true,
        }
    }
}

/// Feature toggles for Discord channel
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordFeatures {
    #[serde(default = "default_true")]
    pub reactions: bool,

    #[serde(default = "default_true")]
    pub slash_commands: bool,

    #[serde(default)]
    pub interactions: bool,

    #[serde(default)]
    pub streaming_preview: bool,

    #[serde(default)]
    pub plural_kit: bool,

    #[serde(default)]
    pub exec_approval: bool,
}

impl Default for DiscordFeatures {
    fn default() -> Self {
        Self {
            reactions: true,
            slash_commands: true,
            interactions: false,
            streaming_preview: false,
            plural_kit: false,
            exec_approval: false,
        }
    }
}

/// Thread binding configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadBindingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allow sub-agents to participate in thread
    #[serde(default)]
    pub allow_sub_agents: bool,

    /// Command prefix for /focus command
    #[serde(default = "default_focus_prefix")]
    pub command_prefix: String,
}

fn default_focus_prefix() -> String {
    "/focus".to_string()
}

impl Default for ThreadBindingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_sub_agents: false,
            command_prefix: "/focus".to_string(),
        }
    }
}

/// Reply configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ReplyConfig {
    /// Auto-reply to threads
    #[serde(default = "default_true")]
    pub auto_reply: bool,

    /// Include quoted original message
    #[serde(default)]
    pub include_quotes: bool,
}

impl Default for ReplyConfig {
    fn default() -> Self {
        Self {
            auto_reply: true,
            include_quotes: false,
        }
    }
}

/// Discord channel configuration with multi-account support
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordChannelConfig {
    /// Default settings applied to all accounts
    #[serde(default)]
    pub default: DiscordChannelSettings,

    /// Account configurations (account_id -> AccountConfig)
    /// Multiple bot instances supported
    #[serde(default)]
    pub accounts: HashMap<String, AccountConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountConfig {
    /// Bot token
    pub token: String,

    /// Application ID (from Discord developer portal)
    pub application_id: u64,

    /// Default settings for this account
    #[serde(default)]
    pub default_settings: DiscordChannelSettings,

    /// Per-guild settings (guild_id -> DiscordGuildSettings)
    #[serde(default)]
    pub guilds: HashMap<u64, DiscordGuildSettings>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordGuildSettings {
    pub name: String,

    /// Guild-level override settings
    #[serde(default)]
    pub settings: DiscordChannelSettings,

    /// Per-channel settings (channel_id -> DiscordChannelSettings)
    #[serde(default)]
    pub channels: HashMap<u64, DiscordChannelSettings>,
}

/// Content retention policy for audit logs
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum ContentRetention {
    Full, // 保留全文
    #[default]
    Anonymized, // 移除 user_id, channel_id
    MetadataOnly, // 仅保留元数据
}

/// Security configuration for Discord channel
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiscordSecurityConfig {
    /// Enable security audit logging
    #[serde(default = "default_true")]
    pub audit_enabled: bool,

    /// Channel IDs to send audit logs to
    #[serde(default)]
    pub audit_channels: Vec<u64>,

    /// Which events to audit
    #[serde(default)]
    pub audit_events: AuditEvents,

    /// Content retention policy
    #[serde(default)]
    pub content_retention: ContentRetention,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuditEvents {
    #[serde(default = "default_true")]
    pub commands: bool,

    #[serde(default)]
    pub exec_approvals: bool,

    #[serde(default)]
    pub message_content: bool,
}

impl Default for AuditEvents {
    fn default() -> Self {
        Self {
            commands: true,
            exec_approvals: true,
            message_content: false,
        }
    }
}

// ============================================================================
// Backward Compatibility
// ============================================================================

impl From<DiscordConfig> for DiscordChannelConfig {
    fn from(legacy: DiscordConfig) -> Self {
        let default_settings = DiscordChannelSettings {
            prefix: Some(legacy.command_prefix),
            allowlist: legacy.allowed_channels,
            blocklist: Vec::new(),
            features: DiscordFeatures {
                reactions: true,
                slash_commands: legacy.slash_commands_enabled,
                interactions: false,
                streaming_preview: false,
                plural_kit: false,
                exec_approval: false,
            },
            thread_binding: ThreadBindingConfig {
                enabled: legacy.threads.enabled,
                allow_sub_agents: false,
                command_prefix: "/focus".to_string(),
            },
            reply: ReplyConfig::default(),
            typing_indicator: legacy.send_typing,
        };

        let account_config = AccountConfig {
            token: legacy.bot_token.clone(),
            application_id: legacy.application_id.unwrap_or(0),
            default_settings,
            guilds: HashMap::new(),
        };

        let mut accounts = HashMap::new();
        accounts.insert("default".to_string(), account_config);

        Self {
            default: DiscordChannelSettings::default(),
            accounts,
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn default_true() -> bool {
    true
}

fn default_prefix() -> String {
    "!".to_string()
}

// ============================================================================
// Legacy Impl
// ============================================================================

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            application_id: None,
            allowed_guilds: Vec::new(),
            allowed_channels: Vec::new(),
            dm_allowed: true,
            command_prefix: "!".to_string(),
            respond_to_mentions: true,
            slash_commands_enabled: true,
            send_typing: true,
            intents: IntentsConfig::default(),
            threads: ThreadConfig::default(),
        }
    }
}

impl DiscordConfig {
    /// Create config from environment variable
    pub fn from_env() -> Option<Self> {
        let bot_token = std::env::var("DISCORD_BOT_TOKEN").ok()?;
        let application_id = std::env::var("DISCORD_APPLICATION_ID")
            .ok()
            .and_then(|s| s.parse().ok());

        Some(Self {
            bot_token,
            application_id,
            ..Default::default()
        })
    }

    /// Check if a guild ID is allowed
    pub fn is_guild_allowed(&self, guild_id: u64) -> bool {
        if self.allowed_guilds.is_empty() {
            true
        } else {
            self.allowed_guilds.contains(&guild_id)
        }
    }

    /// Check if a channel ID is allowed
    pub fn is_channel_allowed(&self, channel_id: u64) -> bool {
        if self.allowed_channels.is_empty() {
            true
        } else {
            self.allowed_channels.contains(&channel_id)
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.bot_token.is_empty() {
            return Err("bot_token is required".to_string());
        }
        // Discord bot tokens are typically 59+ characters
        if self.bot_token.len() < 50 {
            return Err("bot_token appears to be invalid (too short)".to_string());
        }
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DiscordConfig::default();
        assert!(config.bot_token.is_empty());
        assert!(config.dm_allowed);
        assert!(config.slash_commands_enabled);
        assert_eq!(config.command_prefix, "!");
    }

    #[test]
    fn test_guild_allowed() {
        let mut config = DiscordConfig::default();

        // Empty allowed_guilds = allow all
        assert!(config.is_guild_allowed(12345));

        // With allowed_guilds list
        config.allowed_guilds = vec![12345, 67890];
        assert!(config.is_guild_allowed(12345));
        assert!(!config.is_guild_allowed(99999));
    }

    #[test]
    fn test_channel_allowed() {
        let mut config = DiscordConfig::default();

        // Empty allowed_channels = allow all
        assert!(config.is_channel_allowed(12345));

        // With allowed_channels list
        config.allowed_channels = vec![12345];
        assert!(config.is_channel_allowed(12345));
        assert!(!config.is_channel_allowed(99999));
    }

    #[test]
    fn test_validate() {
        let mut config = DiscordConfig::default();

        // Empty token
        assert!(config.validate().is_err());

        // Too short
        config.bot_token = "short".to_string();
        assert!(config.validate().is_err());

        // Valid length (fake token for test)
        config.bot_token =
            "MTIzNDU2Nzg5MDEyMzQ1Njc4OQ.ABcDeF.1234567890abcdefghijklmnop".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_intents_default() {
        let intents = IntentsConfig::default();
        assert!(intents.guild_messages);
        assert!(intents.direct_messages);
        assert!(intents.message_content);
        assert!(!intents.guild_members);
    }

    #[test]
    fn test_discord_channel_settings_default() {
        let settings = DiscordChannelSettings::default();
        assert!(settings.allowlist.is_empty());
        assert!(settings.blocklist.is_empty());
        assert!(settings.typing_indicator);
        assert!(settings.features.reactions);
    }

    #[test]
    fn test_discord_features_default() {
        let features = DiscordFeatures::default();
        assert!(features.reactions);
        assert!(features.slash_commands);
        assert!(!features.interactions);
        assert!(!features.exec_approval);
    }

    #[test]
    fn test_thread_binding_config_default() {
        let config = ThreadBindingConfig::default();
        assert!(config.enabled);
        assert!(!config.allow_sub_agents);
        assert_eq!(config.command_prefix, "/focus");
    }

    #[test]
    fn test_reply_config_default() {
        let config = ReplyConfig::default();
        assert!(config.auto_reply);
        assert!(!config.include_quotes);
    }

    #[test]
    fn test_content_retention_default() {
        let retention = ContentRetention::default();
        assert!(matches!(retention, ContentRetention::Anonymized));
    }

    #[test]
    fn test_audit_events_default() {
        let events = AuditEvents::default();
        assert!(events.commands);
        assert!(events.exec_approvals);
        assert!(!events.message_content);
    }

    #[test]
    fn test_discord_security_config_default() {
        let config = DiscordSecurityConfig::default();
        assert!(!config.audit_enabled);
        assert!(config.audit_channels.is_empty());
        assert!(matches!(
            config.content_retention,
            ContentRetention::Anonymized
        ));
    }

    #[test]
    fn test_backward_compatibility() {
        let legacy = DiscordConfig {
            bot_token: "MTIzNDU2Nzg5MDEyMzQ1Njc4OQ.ABcDeF.1234567890abcdefghijklmnop".to_string(),
            application_id: Some(1234567890),
            allowed_channels: vec![111, 222],
            command_prefix: "!".to_string(),
            slash_commands_enabled: true,
            send_typing: true,
            threads: ThreadConfig {
                enabled: true,
                auto_create: false,
                auto_create_channels: Vec::new(),
            },
            ..Default::default()
        };

        let config: DiscordChannelConfig = legacy.into();

        // Should have one default account
        assert_eq!(config.accounts.len(), 1);

        let account = config.accounts.get("default").unwrap();
        assert_eq!(
            account.token,
            "MTIzNDU2Nzg5MDEyMzQ1Njc4OQ.ABcDeF.1234567890abcdefghijklmnop"
        );
        assert_eq!(account.application_id, 1234567890);

        // Account default settings should come from legacy config
        assert_eq!(account.default_settings.allowlist, vec![111, 222]);
        assert!(account.default_settings.features.slash_commands);
        assert!(account.default_settings.typing_indicator);
    }
}

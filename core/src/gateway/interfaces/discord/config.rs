//! Discord Channel Configuration
//!
//! Configuration types for the Discord Bot integration.

use serde::{Deserialize, Serialize};

/// Discord channel configuration
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
}

impl Default for IntentsConfig {
    fn default() -> Self {
        Self {
            guild_messages: true,
            direct_messages: true,
            message_content: true,
            guild_members: false,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_prefix() -> String {
    "!".to_string()
}

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
        config.bot_token = "MTIzNDU2Nzg5MDEyMzQ1Njc4OQ.ABcDeF.1234567890abcdefghijklmnop".to_string();
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
}

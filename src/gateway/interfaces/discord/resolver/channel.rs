//! Channel Settings Resolver
//!
//! Resolves effective channel settings with per-guild/per-channel override.

use crate::gateway::interfaces::discord::config::{DiscordChannelConfig, DiscordChannelSettings};

/// Resolved channel settings (with override chain applied)
#[derive(Debug, Clone)]
pub struct ResolvedChannelSettings {
    /// The resolved settings
    pub settings: DiscordChannelSettings,
    /// Which account these settings came from
    pub account_id: String,
    /// Which guild these settings came from (if any)
    pub guild_id: Option<u64>,
    /// Which channel these settings came from (if any)
    pub channel_id: Option<u64>,
}

/// Resolves effective channel settings with override chain
pub struct ChannelSettingsResolver {
    config: DiscordChannelConfig,
}

impl ChannelSettingsResolver {
    pub fn new(config: DiscordChannelConfig) -> Self {
        Self { config }
    }

    /// Resolve effective settings for a channel
    ///
    /// Override chain: default -> account -> guild -> channel
    pub fn resolve(
        &self,
        account_id: &str,
        guild_id: Option<u64>,
        channel_id: Option<u64>,
    ) -> ResolvedChannelSettings {
        // Start with global defaults
        let mut settings = self.config.default.clone();

        // Apply account defaults
        if let Some(account) = self.config.accounts.get(account_id) {
            settings = Self::merge_settings(settings, &account.default_settings);
        }

        // Apply guild settings
        if let Some(gid) = guild_id {
            if let Some(account) = self.config.accounts.get(account_id) {
                if let Some(guild) = account.guilds.get(&gid) {
                    settings = Self::merge_settings(settings, &guild.settings);

                    // Apply channel settings
                    if let Some(cid) = channel_id {
                        if let Some(channel) = guild.channels.get(&cid) {
                            settings = Self::merge_settings(settings, channel);
                        }
                    }
                }
            }
        }

        ResolvedChannelSettings {
            settings,
            account_id: account_id.to_string(),
            guild_id,
            channel_id,
        }
    }

    /// Merge two settings, with non-default values in `override` taking precedence
    fn merge_settings(
        base: DiscordChannelSettings,
        override_: &DiscordChannelSettings,
    ) -> DiscordChannelSettings {
        DiscordChannelSettings {
            prefix: override_.prefix.clone().or(base.prefix),
            allowlist: if override_.allowlist.is_empty() {
                base.allowlist
            } else {
                override_.allowlist.clone()
            },
            blocklist: if override_.blocklist.is_empty() {
                base.blocklist
            } else {
                override_.blocklist.clone()
            },
            features: Self::merge_features(&base.features, &override_.features),
            thread_binding: Self::merge_thread_binding(
                &base.thread_binding,
                &override_.thread_binding,
            ),
            reply: Self::merge_reply(&base.reply, &override_.reply),
            typing_indicator: base.typing_indicator,
        }
    }

    fn merge_features(
        _base: &crate::gateway::interfaces::discord::config::DiscordFeatures,
        override_: &crate::gateway::interfaces::discord::config::DiscordFeatures,
    ) -> crate::gateway::interfaces::discord::config::DiscordFeatures {
        crate::gateway::interfaces::discord::config::DiscordFeatures {
            reactions: override_.reactions,
            slash_commands: override_.slash_commands,
            interactions: override_.interactions,
            streaming_preview: override_.streaming_preview,
            plural_kit: override_.plural_kit,
            exec_approval: override_.exec_approval,
        }
    }

    fn merge_thread_binding(
        base: &crate::gateway::interfaces::discord::config::ThreadBindingConfig,
        override_: &crate::gateway::interfaces::discord::config::ThreadBindingConfig,
    ) -> crate::gateway::interfaces::discord::config::ThreadBindingConfig {
        crate::gateway::interfaces::discord::config::ThreadBindingConfig {
            enabled: override_.enabled,
            allow_sub_agents: override_.allow_sub_agents,
            command_prefix: if override_.command_prefix.is_empty() {
                base.command_prefix.clone()
            } else {
                override_.command_prefix.clone()
            },
        }
    }

    fn merge_reply(
        _base: &crate::gateway::interfaces::discord::config::ReplyConfig,
        override_: &crate::gateway::interfaces::discord::config::ReplyConfig,
    ) -> crate::gateway::interfaces::discord::config::ReplyConfig {
        crate::gateway::interfaces::discord::config::ReplyConfig {
            auto_reply: override_.auto_reply,
            include_quotes: override_.include_quotes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_config() -> DiscordChannelConfig {
        let default_settings = DiscordChannelSettings {
            prefix: Some("!".to_string()),
            allowlist: vec![1, 2, 3],
            blocklist: vec![],
            features: crate::gateway::interfaces::discord::config::DiscordFeatures::default(),
            thread_binding:
                crate::gateway::interfaces::discord::config::ThreadBindingConfig::default(),
            reply: crate::gateway::interfaces::discord::config::ReplyConfig::default(),
            typing_indicator: true,
        };

        let account_settings = DiscordChannelSettings {
            prefix: Some("/".to_string()),
            allowlist: vec![],
            blocklist: vec![],
            features: crate::gateway::interfaces::discord::config::DiscordFeatures::default(),
            thread_binding:
                crate::gateway::interfaces::discord::config::ThreadBindingConfig::default(),
            reply: crate::gateway::interfaces::discord::config::ReplyConfig::default(),
            typing_indicator: true,
        };

        let guild_settings = DiscordChannelSettings {
            prefix: None,
            allowlist: vec![10, 11],
            blocklist: vec![],
            features: crate::gateway::interfaces::discord::config::DiscordFeatures::default(),
            thread_binding:
                crate::gateway::interfaces::discord::config::ThreadBindingConfig::default(),
            reply: crate::gateway::interfaces::discord::config::ReplyConfig::default(),
            typing_indicator: true,
        };

        let mut guilds = HashMap::new();
        guilds.insert(
            100u64,
            DiscordGuildSettings {
                name: "Test Guild".to_string(),
                settings: guild_settings,
                channels: HashMap::new(),
            },
        );

        let mut accounts = HashMap::new();
        accounts.insert(
            "test_account".to_string(),
            AccountConfig {
                token: "test_token".to_string(),
                application_id: 123456,
                default_settings: account_settings,
                guilds,
            },
        );

        DiscordChannelConfig {
            default: default_settings,
            accounts,
        }
    }

    #[test]
    fn test_resolve_uses_global_default() {
        let config = create_test_config();
        let resolver = ChannelSettingsResolver::new(config);

        let resolved = resolver.resolve("test_account", None, None);

        // Should use global default prefix "!"
        assert_eq!(resolved.settings.prefix, Some("!".to_string()));
        // Should use global default allowlist
        assert_eq!(resolved.settings.allowlist, vec![1, 2, 3]);
    }

    #[test]
    fn test_resolve_applies_account_override() {
        let config = create_test_config();
        let resolver = ChannelSettingsResolver::new(config);

        let resolved = resolver.resolve("test_account", None, None);

        // Account should override prefix to "/"
        assert_eq!(resolved.settings.prefix, Some("/".to_string()));
    }

    #[test]
    fn test_resolve_applies_guild_override() {
        let config = create_test_config();
        let resolver = ChannelSettingsResolver::new(config);

        let resolved = resolver.resolve("test_account", Some(100), None);

        // Guild should override allowlist to [10, 11]
        assert_eq!(resolved.settings.allowlist, vec![10, 11]);
        // Account prefix "/" should still apply
        assert_eq!(resolved.settings.prefix, Some("/".to_string()));
    }

    #[test]
    fn test_resolve_channel_not_found_returns_guild_settings() {
        let config = create_test_config();
        let resolver = ChannelSettingsResolver::new(config);

        // Channel 999 doesn't exist in guild 100
        let resolved = resolver.resolve("test_account", Some(100), Some(999));

        // Should use guild-level settings
        assert_eq!(resolved.settings.allowlist, vec![10, 11]);
        assert!(resolved.guild_id, Some(100));
        assert!(resolved.channel_id, Some(999));
    }

    #[test]
    fn test_resolve_unknown_account_uses_global_default() {
        let config = create_test_config();
        let resolver = ChannelSettingsResolver::new(config);

        let resolved = resolver.resolve("unknown_account", None, None);

        // Should use global defaults
        assert_eq!(resolved.settings.prefix, Some("!".to_string()));
        assert_eq!(resolved.settings.allowlist, vec![1, 2, 3]);
    }
}

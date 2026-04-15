use crate::gateway::interfaces::discord::config::DiscordChannelConfig;
use std::collections::HashMap;

pub struct AccountResolver {
    channel_to_account: HashMap<u64, String>,
    guild_to_account: HashMap<u64, String>,
}

impl AccountResolver {
    pub fn new(config: &DiscordChannelConfig) -> Self {
        let mut channel_to_account = HashMap::new();
        let mut guild_to_account = HashMap::new();

        for (account_id, account) in &config.accounts {
            for (guild_id, guild) in &account.guilds {
                for channel_id in guild.channels.keys() {
                    channel_to_account.insert(*channel_id, account_id.clone());
                }
                guild_to_account.insert(*guild_id, account_id.clone());
            }
        }

        Self {
            channel_to_account,
            guild_to_account,
        }
    }

    pub fn resolve_account(&self, channel_id: u64) -> Option<String> {
        self.channel_to_account.get(&channel_id).cloned()
    }

    pub fn resolve_account_by_guild(&self, guild_id: u64) -> Option<String> {
        self.guild_to_account.get(&guild_id).cloned()
    }
}

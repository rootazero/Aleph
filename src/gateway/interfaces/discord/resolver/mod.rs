mod error;
mod input;
mod strategy;

pub use error::{Candidate, ChannelResolutionError};
pub use input::ParsedInput;

use super::api::{list_channels, list_guilds, ChannelSummary, GuildSummary};
use crate::sync_primitives::{Arc, Mutex};
use serenity::http::Http;
use strategy::search_in_guilds;

#[derive(Debug, Clone)]
pub struct ResolvedChannel {
    pub channel_id: String,
    pub guild_id: String,
    pub name: String,
}

#[derive(Clone)]
pub struct DiscordResolver {
    http: Arc<Http>,
    cache: Arc<Mutex<Option<Cache>>>,
}

struct Cache {
    channels: Vec<(GuildSummary, Vec<ChannelSummary>)>,
}

impl DiscordResolver {
    pub fn new(http: Arc<Http>) -> Self {
        Self {
            http,
            cache: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn resolve(&self, input: &str) -> Result<ResolvedChannel, ChannelResolutionError> {
        let parsed = input::parse(input)?;
        let guilds_data = self.load_guilds_and_channels().await?;

        let candidates = match &parsed {
            ParsedInput::Id(id) => {
                let mut found = Vec::new();
                for (guild, channels) in &guilds_data {
                    for ch in channels {
                        if ch.channel_id.to_string() == *id {
                            found.push(Candidate {
                                channel_id: ch.channel_id.to_string(),
                                channel_name: ch.name.clone(),
                                guild_id: guild.guild_id.to_string(),
                                guild_name: guild.name.clone(),
                            });
                        }
                    }
                }
                found
            }
            ParsedInput::GuildPrefix { guild, channel } => {
                let target_guild = guilds_data
                    .iter()
                    .find(|(g, _)| to_guild_slug(&g.name) == to_guild_slug(guild));

                match target_guild {
                    Some((guild, channels)) => {
                        search_in_guilds(&[(guild.clone(), channels.clone())], channel)
                    }
                    None => Vec::new(),
                }
            }
            ParsedInput::ChannelName(name) => search_in_guilds(&guilds_data, name),
        };

        match candidates.len() {
            0 => Err(ChannelResolutionError::NotFound(input.to_string())),
            1 => {
                let c = candidates
                    .into_iter()
                    .next()
                    .ok_or_else(|| ChannelResolutionError::NotFound(input.to_string()))?;
                Ok(ResolvedChannel {
                    channel_id: c.channel_id,
                    guild_id: c.guild_id,
                    name: c.channel_name,
                })
            }
            _ => Err(ChannelResolutionError::Ambiguous(candidates)),
        }
    }

    pub async fn list_channels(&self) -> Result<Vec<Candidate>, ChannelResolutionError> {
        let guilds_data = self.load_guilds_and_channels().await?;
        let mut candidates = Vec::new();

        for (guild, channels) in &guilds_data {
            for ch in channels {
                if ch.channel_type != 0 {
                    continue;
                }
                candidates.push(Candidate {
                    channel_id: ch.channel_id.to_string(),
                    channel_name: ch.name.clone(),
                    guild_id: guild.guild_id.to_string(),
                    guild_name: guild.name.clone(),
                });
            }
        }

        Ok(candidates)
    }

    async fn load_guilds_and_channels(
        &self,
    ) -> Result<Vec<(GuildSummary, Vec<ChannelSummary>)>, ChannelResolutionError> {
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref cached) = *cache {
                return Ok(cached.channels.clone());
            }
        }

        let guilds = list_guilds(&self.http)
            .await
            .map_err(ChannelResolutionError::NoPermission)?;

        let mut channels = Vec::new();
        for guild in &guilds {
            match list_channels(&self.http, guild.guild_id).await {
                Ok(chs) => channels.push((guild.clone(), chs)),
                Err(_) => {
                    tracing::debug!("Skipping channels for guild {} (no permission)", guild.name);
                }
            }
        }

        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            *cache = Some(Cache {
                channels: channels.clone(),
            });
        }

        Ok(channels)
    }
}

fn to_guild_slug(name: &str) -> String {
    input::to_slug(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_input_id_bare() {
        let result = input::parse("123");
        assert!(matches!(result, Ok(ParsedInput::Id(ref s)) if s == "123"));
    }

    #[test]
    fn test_parse_input_id_explicit() {
        let result = input::parse("channel:456");
        assert!(matches!(result, Ok(ParsedInput::Id(ref s)) if s == "456"));
    }

    #[test]
    fn test_parse_input_guild_prefix() {
        let result = input::parse("my-guild/general");
        assert!(
            matches!(result, Ok(ParsedInput::GuildPrefix { ref guild, ref channel })
            if guild == "my-guild" && channel == "general")
        );
    }

    #[test]
    fn test_parse_input_channel_name() {
        let result = input::parse("general");
        assert!(matches!(result, Ok(ParsedInput::ChannelName(ref s)) if s == "general"));
    }
}

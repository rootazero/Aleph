//! `WhatsApp` Channel Configuration
//!
//! Configuration for the `WhatsApp` channel adapter.

use crate::gateway::channel_chunking::ChunkMode;
use crate::gateway::channel_policy::{ChannelAccessConfig, DmPolicy, E164Number, GroupPolicy};
use crate::gateway::interfaces::whatsapp::history_buffer::HistoryBufferConfig;
use crate::gateway::interfaces::whatsapp::media::MediaConfig;
use crate::gateway::interfaces::whatsapp::reactions::{AckReactionConfig, ReactionLevel};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    pub phone_number: Option<String>,
    #[serde(default = "default_true")]
    pub send_typing: bool,
    #[serde(default = "default_true")]
    pub mark_read: bool,
    #[serde(default)]
    pub allowed_chats: Vec<String>,
    #[serde(default)]
    pub default_account_id: Option<String>,
    #[serde(default)]
    pub access: ChannelAccessConfig,
    /// Numbers already paired for DMs (E.164; a leading `+` is optional).
    ///
    /// With `dm_policy = "pairing"` a DM from any number absent from this
    /// list is dropped before it reaches the agent, so an unapproved
    /// sender cannot open a conversation.
    #[serde(default)]
    pub paired_numbers: Vec<String>,
    #[serde(default)]
    pub delivery: DeliveryConfig,
    #[serde(default)]
    pub reactions: ReactionConfig,
    #[serde(default)]
    pub media: MediaConfig,
    #[serde(default)]
    pub history: HistoryBufferConfig,
}

const fn default_true() -> bool {
    true
}

impl WhatsAppConfig {
    /// Validate configuration.
    ///
    /// Only rejects values the adapter would otherwise silently misread:
    /// every configured number must be a phone number in the same shape the
    /// `pairing` DM gate compares against the sender JID, i.e. the bare
    /// digits of an international number with an optional leading `+`.
    pub fn validate(&self) -> Result<(), String> {
        let configured = self
            .phone_number
            .iter()
            .map(|number| ("phone_number", number.as_str()))
            .chain(
                self.paired_numbers
                    .iter()
                    .map(|number| ("paired_numbers", number.as_str())),
            );
        for (field, number) in configured {
            // The policy engines compare the number against the local part of
            // the sender JID, so anything `normalize` would rewrite (spaces,
            // dashes, a missing country code) can never match and must be
            // rejected instead of silently ignored.
            let digits: String = number
                .chars()
                .filter(|c: &char| c.is_ascii_digit())
                .collect();
            if E164Number::normalize(number).is_none()
                || digits != number.strip_prefix('+').unwrap_or(number)
            {
                return Err(format!(
                    "{field} entry {number:?} is not an E.164 phone number; \
                     use digits with an optional leading '+'"
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn is_chat_allowed(&self, chat_id: &str) -> bool {
        if self.allowed_chats.is_empty() {
            return true;
        }
        self.allowed_chats.iter().any(|c| c == chat_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessConfig {
    #[serde(default)]
    pub dm_policy: DmPolicy,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub group_policy: GroupPolicy,
    #[serde(default)]
    pub group_allow_from: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Disabled,
            allow_from: Vec::new(),
            group_policy: GroupPolicy::Disabled,
            group_allow_from: Vec::new(),
            groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryConfig {
    #[serde(default = "default_text_chunk_limit")]
    pub text_chunk_limit: usize,
    #[serde(default)]
    pub chunk_mode: ChunkMode,
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,
}

const fn default_text_chunk_limit() -> usize {
    4000
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            text_chunk_limit: 4000,
            chunk_mode: ChunkMode::default(),
            send_read_receipts: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionConfig {
    #[serde(default)]
    pub level: ReactionLevel,
    #[serde(default)]
    pub ack: Option<AckReactionConfig>,
}

impl Default for ReactionConfig {
    fn default() -> Self {
        Self {
            level: ReactionLevel::Minimal,
            ack: Some(AckReactionConfig::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_numbers_the_pairing_gate_could_never_match() {
        let config = WhatsAppConfig {
            paired_numbers: vec!["+1 555 000 1111".to_string()],
            ..WhatsAppConfig::default()
        };
        let err = config.validate().unwrap_err();
        assert!(err.contains("paired_numbers"), "{err}");

        let config = WhatsAppConfig {
            paired_numbers: vec!["+15550001111".to_string()],
            ..WhatsAppConfig::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn default_config_validates_and_allows_all_chats() {
        let config = WhatsAppConfig::default();
        assert!(config.validate().is_ok());
        assert!(config.is_chat_allowed("111@g.us"));
    }
}

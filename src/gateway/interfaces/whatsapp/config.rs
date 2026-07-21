//! `WhatsApp` Channel Configuration
//!
//! Configuration for the `WhatsApp` channel adapter.

use crate::gateway::channel_chunking::ChunkMode;
use crate::gateway::channel_policy::{ChannelAccessConfig, DmPolicy, GroupPolicy};
use crate::gateway::interfaces::whatsapp::history_buffer::HistoryBufferConfig;
use crate::gateway::interfaces::whatsapp::media::MediaConfig;
use crate::gateway::interfaces::whatsapp::reactions::{AckReactionConfig, ReactionLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    pub phone_number: Option<String>,
    pub session_data: Option<String>,
    #[serde(default = "default_true")]
    pub send_typing: bool,
    #[serde(default = "default_true")]
    pub mark_read: bool,
    #[serde(default)]
    pub allowed_chats: Vec<String>,
    #[serde(default)]
    pub default_account_id: Option<String>,
    #[serde(default)]
    pub accounts: Option<HashMap<String, WhatsAppAccountConfig>>,
    #[serde(default)]
    pub access: ChannelAccessConfig,
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
    pub const fn validate(&self) -> Result<(), String> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppAccountConfig {
    pub enabled: bool,
    pub phone_number: Option<String>,
    #[serde(default)]
    pub access: AccessConfig,
    #[serde(default)]
    pub delivery: DeliveryConfig,
    #[serde(default)]
    pub reactions: ReactionConfig,
    #[serde(default)]
    pub media: MediaConfig,
}

impl Default for WhatsAppAccountConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            phone_number: None,
            access: Default::default(),
            delivery: Default::default(),
            reactions: Default::default(),
            media: Default::default(),
        }
    }
}

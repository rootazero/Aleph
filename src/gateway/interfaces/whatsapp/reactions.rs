//! Reaction System for Channel Messages
//!
//! Provides reaction handling with configurable levels and ack reactions.

use crate::gateway::channel::InboundMessage;
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReactionLevel {
    Off,
    #[default]
    Minimal,
    Ack,
    Extensive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckReactionConfig {
    pub emoji: char,
    pub direct: bool,
    pub group: GroupReactionMode,
}

impl Default for AckReactionConfig {
    fn default() -> Self {
        Self {
            emoji: '👀',
            direct: true,
            group: GroupReactionMode::Mentions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupReactionMode {
    #[default]
    Mentions,
    Never,
    Always,
}

#[async_trait::async_trait]
pub trait ReactionSender: Send + Sync {
    async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> Result<(), String>;
}

pub struct ReactionHandler {
    level: ReactionLevel,
    ack_config: Option<AckReactionConfig>,
    sender: Arc<dyn ReactionSender>,
}

impl ReactionHandler {
    pub fn new(
        level: ReactionLevel,
        ack_config: Option<AckReactionConfig>,
        sender: Arc<dyn ReactionSender>,
    ) -> Self {
        Self {
            level,
            ack_config,
            sender,
        }
    }

    pub async fn send_ack(&self, msg: &InboundMessage) -> Result<(), String> {
        if !matches!(
            self.level,
            ReactionLevel::Ack | ReactionLevel::Minimal | ReactionLevel::Extensive
        ) {
            return Ok(());
        }
        let Some(config) = &self.ack_config else {
            return Ok(());
        };
        if msg.is_group {
            if !matches!(config.group, GroupReactionMode::Always) {
                return Ok(());
            }
        } else if !config.direct {
            return Ok(());
        }
        self.sender
            .send_reaction(
                msg.conversation_id.as_str(),
                msg.id.as_str(),
                &config.emoji.to_string(),
            )
            .await
    }

    pub fn should_agent_react(&self, _msg: &InboundMessage) -> bool {
        matches!(
            self.level,
            ReactionLevel::Minimal | ReactionLevel::Extensive
        )
    }
}

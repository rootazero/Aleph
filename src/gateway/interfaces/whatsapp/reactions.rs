//! Reaction System for Channel Messages
//!
//! Provides reaction handling with configurable levels and ack reactions.

use crate::gateway::channel::InboundMessage;
use crate::gateway::interfaces::whatsapp::baileys_runtime::WhatsAppRuntime;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReactionLevel {
    Off,
    #[default]
    Minimal,
    Ack,
    Extensive,
}

/// Acknowledgment reaction configuration
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

pub struct ReactionHandler {
    level: ReactionLevel,
    ack_config: Option<AckReactionConfig>,
    runtime: Arc<dyn WhatsAppRuntime>,
}

impl ReactionHandler {
    pub fn new(
        level: ReactionLevel,
        ack_config: Option<AckReactionConfig>,
        runtime: Arc<dyn WhatsAppRuntime>,
    ) -> Self {
        Self {
            level,
            ack_config,
            runtime,
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

        self.runtime
            .send_reaction(
                msg.conversation_id.as_str(),
                msg.id.as_str(),
                &config.emoji.to_string(),
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn should_agent_react(&self, msg: &InboundMessage) -> bool {
        match self.level {
            ReactionLevel::Off | ReactionLevel::Ack => false,
            ReactionLevel::Minimal => self.should_minimal_react(msg),
            ReactionLevel::Extensive => true,
        }
    }

    fn should_minimal_react(&self, _msg: &InboundMessage) -> bool {
        false
    }
}

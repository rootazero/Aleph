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

    #[must_use]
    pub const fn should_agent_react(&self, _msg: &InboundMessage) -> bool {
        matches!(
            self.level,
            ReactionLevel::Minimal | ReactionLevel::Extensive
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
    use chrono::Utc;

    /// Test double for `ReactionSender` — counts how many `send_reaction` calls
    /// the handler fired and remembers the last emoji / jid / msg id. The
    /// production wiring points `ReactionHandler` at a `WaRuntime` adapter;
    /// here we just want to assert "handler called the sender with the
    /// configured emoji exactly once per inbound message".
    #[derive(Default)]
    struct MockReactionSender {
        calls: tokio::sync::Mutex<Vec<(String, String, String)>>,
    }

    impl MockReactionSender {
        fn new() -> Self {
            Self::default()
        }

        async fn call_count(&self) -> usize {
            self.calls.lock().await.len()
        }

        async fn last_emoji(&self) -> String {
            self.calls
                .lock()
                .await
                .last()
                .map(|c| c.2.clone())
                .unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl ReactionSender for MockReactionSender {
        async fn send_reaction(
            &self,
            jid: &str,
            msg_id: &str,
            emoji: &str,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .await
                .push((jid.to_string(), msg_id.to_string(), emoji.to_string()));
            Ok(())
        }
    }

    fn make_test_inbound_message(conversation_id: &str, is_group: bool) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("whatsapp"),
            conversation_id: ConversationId::new(conversation_id),
            sender_id: UserId::new("user-1"),
            sender_name: Some("Alice".to_string()),
            text: "hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
            metadata: vec![],
        }
    }

    #[tokio::test]
    async fn ack_reaction_sent_on_inbound_message() {
        let sender = Arc::new(MockReactionSender::new());
        let handler = ReactionHandler::new(
            ReactionLevel::Ack,
            Some(AckReactionConfig::default()),
            sender.clone(),
        );
        let msg = make_test_inbound_message("chat-1@g.us", false);
        handler.send_ack(&msg).await.unwrap();
        assert_eq!(sender.call_count().await, 1);
        assert_eq!(sender.last_emoji().await, "\u{1f440}");
    }
}

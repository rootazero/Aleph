//! Group History Buffer for Context Injection
//!
//! Buffers recent group messages for context injection before agent responses.

use crate::gateway::channel::{ConversationId, InboundMessage};
use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferedMessage {
    message: InboundMessage,
    buffered_at: DateTime<Utc>,
}

impl BufferedMessage {
    #[must_use]
    pub fn new(message: InboundMessage) -> Self {
        Self {
            message,
            buffered_at: Utc::now(),
        }
    }

    #[must_use]
    pub fn format_for_context(&self) -> String {
        let sender = self.message.sender_name.as_deref().unwrap_or("Unknown");
        format!(
            "[{}] {}: {}",
            self.buffered_at.format("%H:%M"),
            sender,
            self.message.text
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryBufferConfig {
    pub limit: usize,
    pub inject_context: bool,
    pub inject_delimiter: String,
}

impl Default for HistoryBufferConfig {
    fn default() -> Self {
        Self {
            limit: 50,
            inject_context: true,
            inject_delimiter: "\n".to_string(),
        }
    }
}

pub struct GroupHistoryBuffer {
    config: HistoryBufferConfig,
    buffers: crate::sync_primitives::Arc<
        tokio::sync::RwLock<HashMap<ConversationId, VecDeque<BufferedMessage>>>,
    >,
}

impl GroupHistoryBuffer {
    #[must_use]
    pub fn new(config: HistoryBufferConfig) -> Self {
        Self {
            config,
            buffers: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    pub async fn add(&self, message: &InboundMessage) {
        if !message.is_group {
            return;
        }

        let mut buffers = self.buffers.write().await;
        let queue = buffers
            .entry(message.conversation_id.clone())
            .or_insert_with(VecDeque::new);

        queue.push_back(BufferedMessage::new(message.clone()));

        while queue.len() > self.config.limit {
            queue.pop_front();
        }
    }

    pub async fn get_context(&self, conv_id: &ConversationId) -> Option<String> {
        if !self.config.inject_context {
            return None;
        }

        let buffers = self.buffers.read().await;
        let queue = buffers.get(conv_id)?;

        if queue.is_empty() {
            return None;
        }

        let messages: Vec<String> = queue.iter().map(|b| b.format_for_context()).collect();

        let delimiter = &self.config.inject_delimiter;
        Some(format!(
            "{}{}[Chat messages since your last reply - for context]\n{}{}[Current message - respond to this]",
            messages.join(delimiter),
            delimiter,
            delimiter,
            delimiter
        ))
    }

    pub async fn clear(&self, conv_id: &ConversationId) {
        let mut buffers = self.buffers.write().await;
        buffers.remove(conv_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, MessageId, UserId};
    use chrono::Utc;

    fn make_group_message(conv: &str, text: &str) -> InboundMessage {
        InboundMessage {
            id: MessageId::new(format!("msg-{text}")),
            channel_id: ChannelId::new("whatsapp"),
            conversation_id: ConversationId::new(conv),
            sender_id: UserId::new("user-1"),
            sender_name: Some("Alice".to_string()),
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: true,
            raw: None,
            metadata: vec![],
        }
    }

    /// Two group messages buffered into the same conversation must both
    /// surface in `get_context` so the agent has them when it sees the next
    /// message. Direct-message calls to `add` must be a no-op so we do not
    /// mix private chats into a group's history.
    #[tokio::test]
    async fn group_message_buffered_then_injected() {
        let buffer = GroupHistoryBuffer::new(HistoryBufferConfig::default());
        let conv = ConversationId::new("chat-1@g.us");

        buffer.add(&make_group_message(conv.as_str(), "hello")).await;
        buffer
            .add(&make_group_message(conv.as_str(), "world"))
            .await;

        let ctx = buffer
            .get_context(&conv)
            .await
            .expect("context should be present for non-empty group buffer");
        assert!(
            ctx.contains("hello"),
            "context missing first message: {ctx}"
        );
        assert!(
            ctx.contains("world"),
            "context missing second message: {ctx}"
        );
    }
}

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
    pub fn new(message: InboundMessage) -> Self {
        Self {
            message,
            buffered_at: Utc::now(),
        }
    }

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
    buffers:
        std::sync::Arc<tokio::sync::RwLock<HashMap<ConversationId, VecDeque<BufferedMessage>>>>,
}

impl GroupHistoryBuffer {
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

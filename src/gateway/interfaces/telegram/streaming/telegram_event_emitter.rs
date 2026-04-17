use crate::gateway::channel::{
    ChannelError, ChannelResult, ConversationId, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::event_emitter::{
    EventEmitError, EventEmitter, RunSummary, StreamEvent, ToolResult,
};
use crate::gateway::interfaces::telegram::config_v2::StreamingOptions;
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;
use crate::sync_primitives::{Arc, Ordering};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{LaneId, StreamOrchestrator};

/// Telegram-specific event emitter that routes stream events through the orchestrator.
pub struct TelegramEventEmitter {
    event_tx: mpsc::Sender<StreamEvent>,
    seq_counter: Arc<crate::sync_primitives::AtomicU64>,
    route: crate::gateway::inbound_context::ReplyRoute,
}

impl TelegramEventEmitter {
    pub fn new(
        bot: teloxide::Bot,
        config: StreamingOptions,
        conversation_id: String,
        route: crate::gateway::inbound_context::ReplyRoute,
    ) -> Self {
        let delivery = TelegramDelivery::new(
            bot,
            crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig {
                account_id: "telegram".to_string(),
                bot_token: String::new(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: true,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: config.clone(),
                error_policy: Default::default(),
                max_retries: 3,
            },
            Arc::new(ErrorCooldown::new()),
            conversation_id.clone(),
        );

        let (orchestrator, event_tx) = StreamOrchestrator::new(delivery, config);

        let inbound = crate::gateway::channel::InboundMessage {
            id: MessageId::new("1"),
            conversation_id: ConversationId::new(conversation_id),
            channel_id: route.channel_id.clone(),
            sender_id: crate::gateway::channel::UserId::new("user"),
            sender_name: None,
            text: String::new(),
            timestamp: chrono::Utc::now(),
            attachments: vec![],
            metadata: Default::default(),
            reply_to: route.reply_to.clone(),
            is_group: false,
            raw: None,
        };

        tokio::spawn(orchestrator.run(inbound));

        Self {
            event_tx,
            seq_counter: Arc::new(crate::sync_primitives::AtomicU64::new(0)),
            route,
        }
    }
}

#[async_trait]
impl EventEmitter for TelegramEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.event_tx
            .send(event)
            .await
            .map_err(|_| EventEmitError::ChannelClosed)?;
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_telegram_event_emitter_creation() {
        let emitter = TelegramEventEmitter::new(
            teloxide::Bot::new("test"),
            StreamingOptions::default(),
            "123".to_string(),
            crate::gateway::inbound_context::ReplyRoute::new(
                crate::gateway::channel::ChannelId::new("telegram"),
                crate::gateway::channel::ConversationId::new("123"),
            ),
        );

        emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: "r1".to_string(),
                seq: 1,
                delta: "hi".to_string(),
                full_text: "hi".to_string(),
                content: "hi".to_string(),
                chunk_index: 0,
                is_final: false,
                is_intermediate: false,
            })
            .await
            .unwrap();

        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
    }
}

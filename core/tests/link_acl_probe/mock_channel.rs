//! Mock channel that captures outbound messages for assertion.

use tokio::sync::mpsc;
use async_trait::async_trait;
use chrono::Utc;

use alephcore::gateway::{
    ChannelId, MessageId,
};
use alephcore::gateway::channel::{
    Channel, ChannelCapabilities, ChannelInfo, ChannelResult, ChannelState,
    ChannelStatus, OutboundMessage, SendResult,
};

#[derive(Debug, Clone)]
pub struct CapturedReply {
    pub conversation_id: String,
    pub text: String,
}

pub struct MockChannel {
    info: ChannelInfo,
    state: ChannelState,
    reply_tx: mpsc::UnboundedSender<CapturedReply>,
}

impl MockChannel {
    pub fn new(id: &str, reply_tx: mpsc::UnboundedSender<CapturedReply>) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            channel_type: "mock".to_string(),
            name: id.to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: ChannelCapabilities::default(),
        };
        let state = ChannelState::new(100);
        Self { info, state, reply_tx }
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let _ = self.reply_tx.send(CapturedReply {
            conversation_id: message.conversation_id.as_str().to_string(),
            text: message.text.clone(),
        });
        Ok(SendResult {
            message_id: MessageId::new("sent-1"),
            timestamp: Utc::now(),
        })
    }
}

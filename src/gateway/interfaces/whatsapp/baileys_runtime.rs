//! WhatsApp Runtime Trait
//!
//! Abstract interface for WhatsApp operations enabling testing and mocking.

use crate::gateway::channel::{ChannelResult, OutboundMessage};
use crate::gateway::channel_policy::E164Number;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub phone_number: E164Number,
    pub device_name: String,
    pub wid: String,
    pub connected_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum ReceiptType {
    Delivered,
    Read,
    Played,
}

#[derive(Debug, Clone)]
pub enum WaEvent {
    QrCode {
        data: String,
        expires_at: DateTime<Utc>,
    },
    Connected(ConnectionInfo),
    Disconnected {
        reason: String,
    },
    Message(Box<crate::gateway::channel::InboundMessage>),
    Receipt {
        message_id: String,
        kind: ReceiptType,
    },
    Reaction {
        from: String,
        from_name: Option<String>,
        chat_id: String,
        message_id: String,
        text: String,
        has_reaction: bool,
    },
    Presence {
        jid: String,
        presence: String,
    },
    Error {
        message: String,
    },
}

pub struct SendResponse {
    pub id: String,
}

#[async_trait]
pub trait WhatsAppRuntime: Send + Sync {
    async fn connect(&self) -> ChannelResult<ConnectionInfo>;
    async fn disconnect(&self) -> ChannelResult<()>;
    async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<SendResponse>;
    async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()>;
    async fn mark_read(&self, jid: &str, msg_id: &str) -> ChannelResult<()>;
    async fn send_typing(&self, jid: &str) -> ChannelResult<()>;
    fn connection_info(&self) -> Option<ConnectionInfo>;
}

use async_trait::async_trait;

//! Inbound Message Context
//!
//! Carries routing information through the entire message processing flow.

use serde::{Deserialize, Serialize};

use super::channel::{ChannelId, ConversationId, InboundMessage, MessageId};
use super::router::SessionKey;

/// Route information for sending replies back to the originating conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyRoute {
    /// Channel to send reply through
    pub channel_id: ChannelId,
    /// Conversation to send reply to
    pub conversation_id: ConversationId,
    /// Optional: reply to specific message
    pub reply_to: Option<MessageId>,
}

impl ReplyRoute {
    /// Create a new reply route
    pub fn new(channel_id: ChannelId, conversation_id: ConversationId) -> Self {
        Self {
            channel_id,
            conversation_id,
            reply_to: None,
        }
    }

    /// Create reply route with reply-to reference
    pub fn with_reply_to(mut self, message_id: MessageId) -> Self {
        self.reply_to = Some(message_id);
        self
    }
}

/// Full context for an inbound message, used throughout processing
#[derive(Debug, Clone)]
pub struct InboundContext {
    /// Original inbound message
    pub message: InboundMessage,

    /// Route for sending replies
    pub reply_route: ReplyRoute,

    /// Resolved session key for this message
    pub session_key: SessionKey,

    /// Whether sender is authorized (passed permission check)
    pub is_authorized: bool,

    /// Whether bot was mentioned (for group messages)
    pub is_mentioned: bool,

    /// Sender's normalized identifier
    pub sender_normalized: String,
}

impl InboundContext {
    /// Create a new inbound context
    pub fn new(
        message: InboundMessage,
        reply_route: ReplyRoute,
        session_key: SessionKey,
    ) -> Self {
        let sender_normalized = message.sender_id.as_str().to_string();
        Self {
            message,
            reply_route,
            session_key,
            is_authorized: false,
            is_mentioned: false,
            sender_normalized,
        }
    }

    /// Mark as authorized
    pub fn authorize(mut self) -> Self {
        self.is_authorized = true;
        self
    }

    /// Mark as mentioned
    pub fn with_mention(mut self, mentioned: bool) -> Self {
        self.is_mentioned = mentioned;
        self
    }

    /// Set normalized sender ID
    pub fn with_sender_normalized(mut self, normalized: String) -> Self {
        self.sender_normalized = normalized;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_test_message() -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("imessage"),
            conversation_id: ConversationId::new("+15551234567"),
            sender_id: super::super::channel::UserId::new("+15551234567"),
            sender_name: None,
            text: "Hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    #[test]
    fn test_reply_route_creation() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        assert_eq!(route.channel_id.as_str(), "imessage");
        assert_eq!(route.conversation_id.as_str(), "+15551234567");
        assert!(route.reply_to.is_none());
    }

    #[test]
    fn test_reply_route_with_reply_to() {
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        )
        .with_reply_to(MessageId::new("msg-123"));

        assert_eq!(route.reply_to.as_ref().unwrap().as_str(), "msg-123");
    }

    #[test]
    fn test_inbound_context_creation() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key);

        assert!(!ctx.is_authorized);
        assert!(!ctx.is_mentioned);
        assert_eq!(ctx.sender_normalized, "+15551234567");
    }

    #[test]
    fn test_inbound_context_authorize() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key).authorize();

        assert!(ctx.is_authorized);
    }
}

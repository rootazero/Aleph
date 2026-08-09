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
    /// Optional: the original inbound message ID (for reactions)
    pub inbound_message_id: Option<MessageId>,
}

impl ReplyRoute {
    /// Create a new reply route
    #[must_use]
    pub const fn new(channel_id: ChannelId, conversation_id: ConversationId) -> Self {
        Self {
            channel_id,
            conversation_id,
            reply_to: None,
            inbound_message_id: None,
        }
    }

    /// Create reply route with reply-to reference
    #[must_use]
    pub fn with_reply_to(mut self, message_id: MessageId) -> Self {
        self.reply_to = Some(message_id);
        self
    }

    /// Set the original inbound message ID (for reaction lifecycle)
    #[must_use]
    pub fn with_inbound_message_id(mut self, id: MessageId) -> Self {
        self.inbound_message_id = Some(id);
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

    /// Hint that the reply should be delivered as voice (audio) output
    pub voice_reply_hint: bool,

    /// Whether this turn's user message arrived as ASR-transcribed speech
    /// (set by inbound STT in `voice::inbound`). Distinct from
    /// [`Self::voice_reply_hint`], which also fires for typed messages on a
    /// voice-enabled channel: only `transcribed_input` means the *input* itself
    /// was speech, so only it justifies inviting transcription-artifact repair
    /// in the prompt (`VoiceModeLayer`).
    pub transcribed_input: bool,

    /// Workspace pinned by the route binding that matched this message
    /// (`MatchRule.workspace`). `None` → the channel `default_workspace`
    /// (Layer-1 lock) or the agent default applies. Only set on the
    /// configured-bindings routing path; all other constructors default to
    /// `None` and keep the channel/agent workspace behavior.
    pub workspace: Option<std::path::PathBuf>,
}

impl InboundContext {
    /// Create a new inbound context
    #[must_use]
    pub fn new(message: InboundMessage, reply_route: ReplyRoute, session_key: SessionKey) -> Self {
        let sender_normalized = message.sender_id.as_str().to_string();
        Self {
            message,
            reply_route,
            session_key,
            is_authorized: false,
            is_mentioned: false,
            sender_normalized,
            voice_reply_hint: false,
            transcribed_input: false,
            workspace: None,
        }
    }

    /// Mark as authorized
    #[must_use]
    pub const fn authorize(mut self) -> Self {
        self.is_authorized = true;
        self
    }

    /// Mark as mentioned
    #[must_use]
    pub const fn with_mention(mut self, mentioned: bool) -> Self {
        self.is_mentioned = mentioned;
        self
    }

    /// Set normalized sender ID
    #[must_use]
    pub fn with_sender_normalized(mut self, normalized: String) -> Self {
        self.sender_normalized = normalized;
        self
    }

    /// Set voice reply hint
    #[must_use]
    pub const fn with_voice_reply_hint(mut self, hint: bool) -> Self {
        self.voice_reply_hint = hint;
        self
    }

    /// Mark that this turn's user input was ASR-transcribed speech.
    #[must_use]
    pub const fn with_transcribed_input(mut self, transcribed: bool) -> Self {
        self.transcribed_input = transcribed;
        self
    }

    /// Set the workspace pinned by the matched route binding.
    #[must_use]
    pub fn with_workspace(mut self, workspace: Option<std::path::PathBuf>) -> Self {
        self.workspace = workspace;
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
            metadata: vec![],
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

    #[test]
    fn test_voice_reply_hint_default_false() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key);

        assert!(!ctx.voice_reply_hint);
    }

    #[test]
    fn test_with_voice_reply_hint() {
        let msg = make_test_message();
        let route = ReplyRoute::new(
            ChannelId::new("imessage"),
            ConversationId::new("+15551234567"),
        );
        let session_key = SessionKey::main("main");

        let ctx = InboundContext::new(msg, route, session_key).with_voice_reply_hint(true);

        assert!(ctx.voice_reply_hint);
    }
}

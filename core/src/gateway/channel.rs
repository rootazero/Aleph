//! Channel Abstraction Layer
//!
//! Provides a unified interface for multi-channel messaging (iMessage, Telegram, Slack, etc.)
//! Based on Moltbot's plugin-based channel architecture with adapter composition.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    ChannelRegistry                       │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐    │
//! │  │ iMessage│  │Telegram │  │  Slack  │  │ Discord │    │
//! │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘    │
//! │       │            │            │            │          │
//! │       └────────────┴────────────┴────────────┘          │
//! │                         │                                │
//! │                    EventBus                              │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Concepts
//!
//! - **Channel**: A messaging platform adapter (iMessage, Telegram, etc.)
//! - **InboundMessage**: Message received from a channel
//! - **OutboundMessage**: Message to be sent through a channel
//! - **ChannelCapabilities**: What a channel supports (attachments, reactions, etc.)

use std::collections::{HashMap, HashSet};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::thinker::interaction::{Capability, InteractionManifest};

/// Result type for channel operations
pub type ChannelResult<T> = Result<T, ChannelError>;

/// Errors that can occur in channel operations
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("Channel not connected: {0}")]
    NotConnected(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Rate limited: retry after {retry_after_secs} seconds")]
    RateLimited { retry_after_secs: u64 },

    #[error("Message too large: {size} bytes (max: {max_size})")]
    MessageTooLarge { size: usize, max_size: usize },

    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),

    #[error("Send failed: {0}")]
    SendFailed(String),

    #[error("Receive failed: {0}")]
    ReceiveFailed(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Channel identifier
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelId(pub String);

impl ChannelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Conversation identifier within a channel
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl ConversationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// User identifier within a channel
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Message identifier within a channel
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Inline keyboard button
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineButton {
    /// Button text
    pub text: String,
    /// Callback data (sent back when clicked)
    pub callback_data: String,
}

/// Inline keyboard row (buttons in a row)
pub type InlineKeyboardRow = Vec<InlineButton>;

/// Inline keyboard markup
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InlineKeyboard {
    /// Rows of buttons
    pub rows: Vec<InlineKeyboardRow>,
}

impl InlineKeyboard {
    /// Create empty keyboard
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// Add a row of buttons
    pub fn row(mut self, buttons: Vec<InlineButton>) -> Self {
        self.rows.push(buttons);
        self
    }

    /// Add a single button as a new row
    pub fn button(self, text: impl Into<String>, callback_data: impl Into<String>) -> Self {
        self.row(vec![InlineButton {
            text: text.into(),
            callback_data: callback_data.into(),
        }])
    }
}

/// Callback query from inline keyboard button click
#[derive(Debug, Clone)]
pub struct CallbackQuery {
    /// Unique query ID (use to answer the callback)
    pub id: String,
    /// User who clicked the button
    pub user_id: UserId,
    /// Chat where the button was clicked
    pub chat_id: ConversationId,
    /// Message containing the button
    pub message_id: MessageId,
    /// Callback data from the button
    pub data: String,
}

/// Attachment in a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Unique identifier for this attachment
    pub id: String,
    /// MIME type (e.g., "image/png", "audio/mp3")
    pub mime_type: String,
    /// Filename if available
    pub filename: Option<String>,
    /// File size in bytes
    pub size: Option<u64>,
    /// URL to download the attachment (if remote)
    pub url: Option<String>,
    /// Local file path (if local)
    pub path: Option<String>,
    /// Inline data (for small attachments)
    pub data: Option<Vec<u8>>,
}

/// Message received from a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Unique message ID from the channel
    pub id: MessageId,
    /// Channel this message came from
    pub channel_id: ChannelId,
    /// Conversation/chat ID
    pub conversation_id: ConversationId,
    /// Sender's user ID
    pub sender_id: UserId,
    /// Sender's display name
    pub sender_name: Option<String>,
    /// Message text content
    pub text: String,
    /// Attachments (images, files, etc.)
    pub attachments: Vec<Attachment>,
    /// When the message was sent
    pub timestamp: DateTime<Utc>,
    /// Message this is replying to (if any)
    pub reply_to: Option<MessageId>,
    /// Whether this is a group message
    pub is_group: bool,
    /// Raw message data from the channel (for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Message to be sent through a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    /// Target conversation ID
    pub conversation_id: ConversationId,
    /// Message text content
    pub text: String,
    /// Attachments to send
    pub attachments: Vec<Attachment>,
    /// Message to reply to (if any)
    pub reply_to: Option<MessageId>,
    /// Optional inline keyboard (for platforms that support it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_keyboard: Option<InlineKeyboard>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl OutboundMessage {
    /// Create a simple text message
    pub fn text(conversation_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            conversation_id: ConversationId::new(conversation_id),
            text: text.into(),
            attachments: Vec::new(),
            reply_to: None,
            inline_keyboard: None,
            metadata: HashMap::new(),
        }
    }

    /// Add a reply-to reference
    pub fn with_reply_to(mut self, message_id: impl Into<String>) -> Self {
        self.reply_to = Some(MessageId::new(message_id));
        self
    }

    /// Add an attachment
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }
}

/// Result of sending a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    /// ID assigned to the sent message by the channel
    pub message_id: MessageId,
    /// When the message was sent
    pub timestamp: DateTime<Utc>,
}

/// What features a channel supports
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelCapabilities {
    /// Supports file attachments
    pub attachments: bool,
    /// Supports image attachments
    pub images: bool,
    /// Supports audio attachments
    pub audio: bool,
    /// Supports video attachments
    pub video: bool,
    /// Supports message reactions
    pub reactions: bool,
    /// Supports message replies/threading
    pub replies: bool,
    /// Supports message editing
    pub editing: bool,
    /// Supports message deletion
    pub deletion: bool,
    /// Supports typing indicators
    pub typing_indicator: bool,
    /// Supports read receipts
    pub read_receipts: bool,
    /// Supports rich text/markdown
    pub rich_text: bool,
    /// Maximum message length (0 = unlimited)
    pub max_message_length: usize,
    /// Maximum attachment size in bytes (0 = unlimited)
    pub max_attachment_size: u64,
}

/// Channel connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelStatus {
    /// Not yet connected
    Disconnected,
    /// Connecting in progress
    Connecting,
    /// Connected and ready
    Connected,
    /// Connection failed, may retry
    Error,
    /// Permanently disabled
    Disabled,
}

/// Channel information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// Channel identifier
    pub id: ChannelId,
    /// Human-readable name
    pub name: String,
    /// Channel type (e.g., "telegram", "imessage", "slack")
    pub channel_type: String,
    /// Current connection status
    pub status: ChannelStatus,
    /// Channel capabilities
    pub capabilities: ChannelCapabilities,
}

/// Pairing data for a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PairingData {
    /// No pairing data available
    None,
    /// Alphanumeric pairing code
    Code(String),
    /// QR code (usually base64 encoded image or URL)
    QrCode(String),
}

/// The main Channel trait - all channel implementations must implement this
#[async_trait]
pub trait Channel: Send + Sync {
    /// Get channel information
    fn info(&self) -> &ChannelInfo;

    /// Get channel ID
    fn id(&self) -> &ChannelId {
        &self.info().id
    }

    /// Get channel type
    fn channel_type(&self) -> &str {
        &self.info().channel_type
    }

    /// Get current status
    fn status(&self) -> ChannelStatus {
        self.info().status
    }

    /// Get capabilities
    fn capabilities(&self) -> &ChannelCapabilities {
        &self.info().capabilities
    }

    /// Get pairing data (e.g., QR code for WhatsApp or pairing code for iMessage)
    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        Ok(PairingData::None)
    }

    /// Start the channel (connect, authenticate, etc.)
    async fn start(&mut self) -> ChannelResult<()>;

    /// Stop the channel (disconnect, cleanup)
    async fn stop(&mut self) -> ChannelResult<()>;

    /// Send a message through this channel
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;

    /// Get the inbound message receiver
    ///
    /// Returns a receiver for incoming messages. The channel implementation
    /// is responsible for populating this channel with messages.
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>>;

    /// Send a typing indicator
    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        if !self.capabilities().typing_indicator {
            return Err(ChannelError::UnsupportedFeature("typing indicator".to_string()));
        }
        // Default implementation does nothing
        let _ = conversation_id;
        Ok(())
    }

    /// Mark a message as read
    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> {
        if !self.capabilities().read_receipts {
            return Err(ChannelError::UnsupportedFeature("read receipts".to_string()));
        }
        // Default implementation does nothing
        let _ = message_id;
        Ok(())
    }

    /// React to a message
    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        if !self.capabilities().reactions {
            return Err(ChannelError::UnsupportedFeature("reactions".to_string()));
        }
        // Default implementation does nothing
        let _ = (message_id, reaction);
        Ok(())
    }

    /// Edit a previously sent message
    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        if !self.capabilities().editing {
            return Err(ChannelError::UnsupportedFeature("editing".to_string()));
        }
        // Default implementation does nothing
        let _ = (message_id, new_text);
        Ok(())
    }

    /// Delete a message
    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        if !self.capabilities().deletion {
            return Err(ChannelError::UnsupportedFeature("deletion".to_string()));
        }
        // Default implementation does nothing
        let _ = message_id;
        Ok(())
    }
}

/// Provider of interaction manifest for a channel
///
/// Channels implement this to declare their interaction capabilities.
/// The manifest is used by ContextAggregator to filter tools and
/// generate appropriate system prompts.
pub trait ChannelProvider {
    /// Get the interaction manifest for this channel
    fn interaction_manifest(&self) -> InteractionManifest;

    /// Optional runtime capability detection
    ///
    /// Override this to detect capabilities at runtime (e.g., terminal features).
    /// Returns None by default.
    fn detect_capabilities(&self) -> Option<HashSet<Capability>> {
        None
    }
}

/// Factory for creating channel instances
#[async_trait]
pub trait ChannelFactory: Send + Sync {
    /// Channel type this factory creates
    fn channel_type(&self) -> &str;

    /// Create a channel instance from configuration
    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>>;
}

/// Configuration for a channel instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Unique identifier for this channel instance
    pub id: String,
    /// Channel type (e.g., "telegram", "imessage")
    pub channel_type: String,
    /// Whether this channel is enabled
    pub enabled: bool,
    /// Channel-specific configuration
    pub config: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_message_builder() {
        let msg = OutboundMessage::text("conv-123", "Hello world")
            .with_reply_to("msg-456");

        assert_eq!(msg.conversation_id.as_str(), "conv-123");
        assert_eq!(msg.text, "Hello world");
        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "msg-456");
    }

    #[test]
    fn test_channel_id() {
        let id = ChannelId::new("telegram:12345");
        assert_eq!(id.as_str(), "telegram:12345");
        assert_eq!(format!("{}", id), "telegram:12345");
    }

    #[test]
    fn test_capabilities_default() {
        let caps = ChannelCapabilities::default();
        assert!(!caps.attachments);
        assert!(!caps.reactions);
        assert_eq!(caps.max_message_length, 0);
    }

    #[test]
    fn test_inline_keyboard_builder() {
        let keyboard = InlineKeyboard::new()
            .row(vec![
                InlineButton { text: "Allow Once".into(), callback_data: "approve:abc:once".into() },
                InlineButton { text: "Allow Always".into(), callback_data: "approve:abc:always".into() },
            ])
            .button("Deny", "approve:abc:deny");

        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(keyboard.rows[0].len(), 2);
        assert_eq!(keyboard.rows[1].len(), 1);
    }
}

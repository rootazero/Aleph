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

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::gateway::cancellation::CancellationToken;
use crate::gateway::channel_approval::ChannelApprovalCapability;
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

/// Platform-specific metadata attached to an inbound message.
///
/// Each variant carries one piece of channel-specific context that the
/// generic pipeline may act on (e.g. grouping media, detecting forwards).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageMeta {
    /// Telegram `media_group_id` — messages sharing this ID belong to one album.
    MediaGroupId(String),
    /// The message was forwarded from another conversation.
    ForwardOrigin {
        sender_name: Option<String>,
        date: Option<DateTime<Utc>>,
    },
    /// Slack `app_mention` event — message contains a bot mention.
    /// This bypasses mention-gating since Slack has already validated the mention.
    AppMention,
    /// MS Teams quoted reply — message is a reply quoting this content.
    Quote {
        /// Display name of the quoted message author.
        sender: Option<String>,
        /// Plain-text body of the quoted message.
        body: String,
    },
    /// Matrix thread root event ID — message is part of a thread.
    ThreadRoot(MessageId),
    /// Telegram poll answer — user selected options in a poll.
    PollAnswer { poll_id: String, option_ids: Vec<u8> },
    /// Telegram message reaction — user reacted with these emojis.
    Reaction { emojis: Vec<String> },
    ThreadId(i64),
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
    /// Platform-specific metadata (media groups, forward info, etc.)
    #[serde(default)]
    pub metadata: Vec<MessageMeta>,
}

impl InboundMessage {
    /// Returns the `media_group_id` if present in metadata.
    pub fn meta_media_group_id(&self) -> Option<&str> {
        self.metadata.iter().find_map(|m| match m {
            MessageMeta::MediaGroupId(id) => Some(id.as_str()),
            _ => None,
        })
    }

    /// Returns `true` if this message was forwarded from another conversation.
    pub fn is_forwarded(&self) -> bool {
        self.metadata
            .iter()
            .any(|m| matches!(m, MessageMeta::ForwardOrigin { .. }))
    }

    /// Returns the quoted reply content if present in metadata.
    pub fn meta_quote(&self) -> Option<(&Option<String>, &str)> {
        self.metadata.iter().find_map(|m| match m {
            MessageMeta::Quote { sender, body } => Some((sender, body.as_str())),
            _ => None,
        })
    }
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
    /// Streaming protocol supported by this channel
    #[serde(default)]
    pub stream_protocol: StreamProtocol,
}

/// Streaming protocol supported by a channel
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProtocol {
    /// No streaming — buffer and send on completion
    #[default]
    None,
    /// Send initial message, then repeatedly edit it (Telegram, Discord)
    EditBased,
    /// Channel handles streaming natively (Teams streaminfo)
    Native,
}

/// Channel connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelStatus {
    /// Not yet connected
    Disconnected,
    /// Connecting in progress
    Connecting,
    /// Waiting for QR scan / pairing
    Pairing,
    /// Connected and ready
    Connected,
    /// Connection failed, may retry
    Error,
    /// Permanently disabled
    Disabled,
}

/// Health status of a channel indicating if it's stale or healthy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Channel is healthy and receiving events
    Healthy,
    /// Channel has not received events within the stale threshold
    Stale,
    /// Channel has recorded failures
    Degraded,
}

/// Channel health tracking for auto-recovery decisions
///
/// Tracks when a channel last received an event, consecutive failure count,
/// and the reason for the current degraded state. Used by the health monitor
/// to determine when to trigger auto-restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelHealth {
    /// Timestamp of the last event received through this channel
    pub last_event_at: DateTime<Utc>,
    /// Number of consecutive failures since last successful event
    pub failure_count: u32,
    /// Human-readable reason for degraded state (if any)
    pub status_reason: Option<String>,
    /// Current health evaluation
    pub status: HealthStatus,
}

impl ChannelHealth {
    /// Create a new health tracker with the current timestamp
    pub fn new() -> Self {
        Self {
            last_event_at: Utc::now(),
            failure_count: 0,
            status_reason: None,
            status: HealthStatus::Healthy,
        }
    }

    /// Record that an event was successfully received
    pub fn record_event(&mut self) {
        self.last_event_at = Utc::now();
        self.failure_count = 0;
        self.status_reason = None;
        self.status = HealthStatus::Healthy;
    }

    /// Record a failure with optional reason
    pub fn record_failure(&mut self, reason: Option<String>) {
        self.failure_count = self.failure_count.saturating_add(1);
        if self.failure_count > 0 {
            self.status = HealthStatus::Degraded;
            if self.status_reason.is_none() {
                self.status_reason = reason;
            }
        }
    }

    /// Check if the channel is stale (no events within threshold)
    pub fn is_stale(&self, stale_threshold: chrono::Duration) -> bool {
        Utc::now() - self.last_event_at > stale_threshold
    }

    /// Update health status based on stale threshold
    pub fn update_status(&mut self, stale_threshold: chrono::Duration) {
        if self.failure_count > 0 {
            self.status = HealthStatus::Degraded;
        } else if self.is_stale(stale_threshold) {
            self.status = HealthStatus::Stale;
        } else {
            self.status = HealthStatus::Healthy;
        }
    }
}

impl Default for ChannelHealth {
    fn default() -> Self {
        Self::new()
    }
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

/// Shared mutable state for Channel implementations.
///
/// Encapsulates thread-safe status tracking and inbound message broadcast.
/// Every Channel should embed this and return it from `fn state()`.
/// Wrapper around broadcast::Sender that provides mpsc-compatible send() for
/// backward compatibility with existing channel implementations.
#[derive(Debug, Clone)]
pub struct InboundMessageSender {
    inner: broadcast::Sender<InboundMessage>,
}

impl InboundMessageSender {
    /// Send a message to all subscribers. Returns `Err(msg)` if no subscribers.
    #[allow(clippy::result_large_err)]
    pub fn send(&self, msg: InboundMessage) -> Result<(), InboundMessage> {
        self.inner.send(msg).map(|_| ()).map_err(|e| e.0)
    }
}

impl From<broadcast::Sender<InboundMessage>> for InboundMessageSender {
    fn from(inner: broadcast::Sender<InboundMessage>) -> Self {
        Self { inner }
    }
}

pub struct ChannelState {
    /// Thread-safe status — set by start()/stop(), read by status()
    status: Arc<tokio::sync::RwLock<ChannelStatus>>,
    /// Broadcast sender — channel impl pushes inbound messages here
    /// Multiple consumers can subscribe via `inbound_subscribe()`
    inbound_broadcast: broadcast::Sender<InboundMessage>,
    /// Cancellation token for graceful shutdown
    cancel: CancellationToken,
    health: Arc<tokio::sync::RwLock<ChannelHealth>>,
}

impl ChannelState {
    /// Create with initial Disconnected status and a broadcast channel.
    pub fn new(buffer_size: usize) -> Self {
        let (tx, _) = broadcast::channel(buffer_size);
        Self {
            status: Arc::new(tokio::sync::RwLock::new(ChannelStatus::Disconnected)),
            inbound_broadcast: tx,
            cancel: CancellationToken::new(),
            health: Arc::new(tokio::sync::RwLock::new(ChannelHealth::new())),
        }
    }

    pub async fn health(&self) -> ChannelHealth {
        self.health.read().await.clone()
    }

    pub async fn record_event(&self) {
        self.health.write().await.record_event();
    }

    pub async fn record_failure(&self, reason: Option<String>) {
        self.health.write().await.record_failure(reason);
    }

    pub fn health_handle(&self) -> Arc<tokio::sync::RwLock<ChannelHealth>> {
        self.health.clone()
    }

    /// Read current status (non-blocking via try_read, fallback Connecting).
    pub fn status(&self) -> ChannelStatus {
        self.status
            .try_read()
            .map(|s| *s)
            .unwrap_or(ChannelStatus::Connecting)
    }

    /// Set status (async, takes write lock).
    pub async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    pub fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        self.inbound_broadcast.subscribe()
    }

    pub fn send_inbound(&self, message: InboundMessage) -> bool {
        self.inbound_broadcast.send(message).is_ok()
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn status_handle(&self) -> Arc<tokio::sync::RwLock<ChannelStatus>> {
        self.status.clone()
    }

    pub fn sender(&self) -> InboundMessageSender {
        InboundMessageSender::from(self.inbound_broadcast.clone())
    }

    pub fn take_receiver(&self) -> Option<broadcast::Receiver<InboundMessage>> {
        Some(self.inbound_broadcast.subscribe())
    }
}

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

    /// Get shared mutable state (status + inbound channel)
    fn state(&self) -> &ChannelState;

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
        self.state().status()
    }

    async fn health(&self) -> ChannelHealth {
        self.state().health().await
    }

    /// Get capabilities
    fn capabilities(&self) -> &ChannelCapabilities {
        &self.info().capabilities
    }

    /// Get approval capability for this channel.
    ///
    /// Returns the channel's approval delivery capability if supported.
    /// Channels that don't support approval delivery return `None`.
    fn approval_capability(&self) -> Option<Arc<dyn ChannelApprovalCapability>> {
        None
    }

    /// Get pairing data (e.g., QR code for WhatsApp or pairing code for iMessage)
    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        Ok(PairingData::None)
    }

    /// List active (non-expired) pairing codes for this channel.
    ///
    /// Returns a list of `(code, remaining_ttl_secs)` pairs.
    /// Default implementation returns an empty list (channel doesn't support pairing).
    async fn list_active_pairing_codes(&self) -> ChannelResult<Vec<(String, u64)>> {
        Ok(Vec::new())
    }

    /// Start the channel (connect, authenticate, etc.)
    async fn start(&mut self) -> ChannelResult<()>;

    /// Stop the channel (disconnect, cleanup)
    async fn stop(&mut self) -> ChannelResult<()>;

    /// Send a message through this channel
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;

    /// Returns a subscriber receiver for incoming messages. Multiple subscribers
    /// can call this simultaneously — each gets their own broadcast receiver.
    fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        self.state().inbound_subscribe()
    }

    /// Send a typing indicator
    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        if !self.capabilities().typing_indicator {
            return Err(ChannelError::UnsupportedFeature(
                "typing indicator".to_string(),
            ));
        }
        // Default implementation does nothing
        let _ = conversation_id;
        Ok(())
    }

    /// Mark a message as read
    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> {
        if !self.capabilities().read_receipts {
            return Err(ChannelError::UnsupportedFeature(
                "read receipts".to_string(),
            ));
        }
        // Default implementation does nothing
        let _ = message_id;
        Ok(())
    }

    /// React to a message
    async fn react(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        if !self.capabilities().reactions {
            return Err(ChannelError::UnsupportedFeature("reactions".to_string()));
        }
        // Default implementation does nothing
        let _ = (conversation_id, message_id, reaction);
        Ok(())
    }

    /// Edit a previously sent message
    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        if !self.capabilities().editing {
            return Err(ChannelError::UnsupportedFeature("editing".to_string()));
        }
        // Default implementation does nothing
        let _ = (conversation_id, message_id, new_text);
        Ok(())
    }

    /// Delete a message
    async fn delete(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        if !self.capabilities().deletion {
            return Err(ChannelError::UnsupportedFeature("deletion".to_string()));
        }
        // Default implementation does nothing
        let _ = (conversation_id, message_id);
        Ok(())
    }

    /// Get native stream handler (if channel supports StreamProtocol::Native)
    fn native_stream_handler(&self) -> Option<Arc<dyn NativeStreamHandler>> {
        None
    }
}

/// Handler for channels that support native streaming protocols (e.g., Teams streaminfo)
#[async_trait]
pub trait NativeStreamHandler: Send + Sync {
    /// Start streaming — send initial status indicator, returns stream_id
    async fn stream_start(
        &self,
        conversation_id: &ConversationId,
        status_text: &str,
    ) -> ChannelResult<String>;
    /// Send accumulated text as streaming update
    async fn stream_update(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        text: &str,
        sequence: u32,
    ) -> ChannelResult<()>;
    /// Finalize stream with complete message
    async fn stream_finalize(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult>;
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
        let msg = OutboundMessage::text("conv-123", "Hello world").with_reply_to("msg-456");

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
                InlineButton {
                    text: "Allow Once".into(),
                    callback_data: "approve:abc:once".into(),
                },
                InlineButton {
                    text: "Allow Always".into(),
                    callback_data: "approve:abc:always".into(),
                },
            ])
            .button("Deny", "approve:abc:deny");

        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(keyboard.rows[0].len(), 2);
        assert_eq!(keyboard.rows[1].len(), 1);
    }

    #[tokio::test]
    async fn test_channel_state_initial_status() {
        let state = ChannelState::new(100);
        assert_eq!(state.status(), ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_channel_state_set_status() {
        let state = ChannelState::new(100);
        state.set_status(ChannelStatus::Connected).await;
        assert_eq!(state.status(), ChannelStatus::Connected);
    }

    #[tokio::test]
    async fn test_channel_state_take_receiver_once() {
        let state = ChannelState::new(100);
        // Broadcast semantics: every call subscribes a fresh receiver.
        let rx = state.take_receiver();
        assert!(rx.is_some());
        let rx2 = state.take_receiver();
        assert!(rx2.is_some());
    }

    #[tokio::test]
    async fn test_channel_state_sender_delivers() {
        let state = ChannelState::new(100);
        let mut rx = state.take_receiver().unwrap();
        let tx = state.sender();

        let msg = InboundMessage {
            id: MessageId::new("test-1"),
            channel_id: ChannelId::new("test"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: "hello".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };

        tx.send(msg).unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.text, "hello");
    }

    #[tokio::test]
    async fn test_channel_health_initial_state() {
        let health = ChannelHealth::new();
        assert_eq!(health.failure_count, 0);
        assert!(health.status_reason.is_none());
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_channel_health_record_event() {
        let mut health = ChannelHealth::new();
        health.record_failure(Some("test failure".to_string()));
        assert!(health.failure_count > 0);
        assert_eq!(health.status, HealthStatus::Degraded);

        health.record_event();
        assert_eq!(health.failure_count, 0);
        assert!(health.status_reason.is_none());
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_channel_health_record_failure() {
        let mut health = ChannelHealth::new();
        health.record_failure(Some("connection timeout".to_string()));
        assert_eq!(health.failure_count, 1);
        assert_eq!(health.status_reason.as_deref(), Some("connection timeout"));
        assert_eq!(health.status, HealthStatus::Degraded);

        health.record_failure(Some("retry failed".to_string()));
        assert_eq!(health.failure_count, 2);
        assert_eq!(health.status_reason.as_deref(), Some("connection timeout"));
    }

    #[tokio::test]
    async fn test_channel_health_is_stale() {
        let mut health = ChannelHealth::new();
        let stale_threshold = chrono::Duration::minutes(5);

        assert!(!health.is_stale(stale_threshold));

        health.last_event_at = Utc::now() - chrono::Duration::minutes(10);
        assert!(health.is_stale(stale_threshold));
    }

    #[tokio::test]
    async fn test_channel_state_health_tracking() {
        let state = ChannelState::new(100);

        let initial_health = state.health().await;
        assert_eq!(initial_health.failure_count, 0);
        assert_eq!(initial_health.status, HealthStatus::Healthy);

        state.record_failure(Some("test error".to_string())).await;
        let after_failure = state.health().await;
        assert_eq!(after_failure.failure_count, 1);
        assert_eq!(after_failure.status, HealthStatus::Degraded);

        state.record_event().await;
        let after_event = state.health().await;
        assert_eq!(after_event.failure_count, 0);
        assert_eq!(after_event.status, HealthStatus::Healthy);
    }
}

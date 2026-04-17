//! Telegram Webhook Handler
//!
//! Implements the [`WebhookHandler`] trait for receiving Telegram updates via webhook
//! instead of long-polling. This allows the bot to respond faster and reduces API calls.
//!
//! # Security
//!
//! Telegram webhooks are verified using the `X-Telegram-Bot-Api-Secret-Token` header.
//! This header value must match the configured secret token.

use crate::gateway::channel::{
    CallbackQuery, ChannelError, ChannelId, ChannelResult, ConversationId, InboundMessage,
    MessageId, MessageMeta, UserId,
};
use crate::gateway::webhook_receiver::WebhookHandler;
use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::mpsc;

/// Secret token header name used by Telegram
const TELEGRAM_SECRET_TOKEN_HEADER: &str = "X-Telegram-Bot-Api-Secret-Token";

/// Telegram webhook handler for the shared WebhookReceiver HTTP server.
///
/// This handler receives raw Telegram Update JSON payloads via HTTP POST,
/// parses them into [`InboundMessage`] objects, and forwards them to the
/// channel's inbound message sender.
pub struct TelegramWebhookHandler {
    /// Bot token for API calls (used for file URL resolution)
    bot_token: String,
    /// Channel ID to tag messages with
    channel_id: ChannelId,
    /// URL path to register (e.g., "/webhook/telegram")
    path: String,
    /// Secret token for webhook verification
    secret_token: Option<String>,
    /// Inbound message sender (forward parsed messages here)
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Callback query sender
    callback_tx: mpsc::Sender<CallbackQuery>,
    /// Access control config
    allowed_users: Vec<i64>,
    /// Access control config
    allowed_groups: Vec<i64>,
}

impl TelegramWebhookHandler {
    /// Create a new TelegramWebhookHandler
    pub fn new(
        bot_token: String,
        channel_id: ChannelId,
        path: String,
        secret_token: Option<String>,
        inbound_tx: mpsc::Sender<InboundMessage>,
        callback_tx: mpsc::Sender<CallbackQuery>,
        allowed_users: Vec<i64>,
        allowed_groups: Vec<i64>,
    ) -> Self {
        Self {
            bot_token,
            channel_id,
            path,
            secret_token,
            inbound_tx,
            callback_tx,
            allowed_users,
            allowed_groups,
        }
    }

    /// Check if a user is allowed to send messages
    fn is_user_allowed(&self, user_id: i64) -> bool {
        if self.allowed_users.is_empty() {
            true
        } else {
            self.allowed_users.contains(&user_id)
        }
    }

    /// Check if a group is allowed
    fn is_group_allowed(&self, chat_id: i64) -> bool {
        if self.allowed_groups.is_empty() {
            true
        } else {
            self.allowed_groups.contains(&chat_id)
        }
    }
}

#[async_trait]
impl WebhookHandler for TelegramWebhookHandler {
    /// Verify the webhook request using the Telegram secret token.
    ///
    /// Telegram sends the secret token in the `X-Telegram-Bot-Api-Secret-Token` header.
    /// If no secret token is configured, verification passes (for development).
    fn verify(&self, headers: &HeaderMap, _body: &[u8]) -> bool {
        // If no secret configured, skip verification (development mode)
        let Some(expected_secret) = &self.secret_token else {
            tracing::debug!("Telegram webhook: no secret configured, skipping verification");
            return true;
        };

        // Extract the secret token from headers
        let actual_secret = headers
            .get(TELEGRAM_SECRET_TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Constant-time comparison to prevent timing attacks
        if actual_secret.is_empty() {
            tracing::warn!("Telegram webhook: missing secret token header");
            return false;
        }

        // Use constant-time comparison
        use subtle::ConstantTimeEq;
        let result = expected_secret.as_bytes().ct_eq(actual_secret.as_bytes());
        if !result {
            tracing::warn!("Telegram webhook: secret token mismatch");
        }
        result.into()
    }

    /// Parse a Telegram Update payload into InboundMessages.
    async fn handle(
        &self,
        _headers: &HeaderMap,
        body: bytes::Bytes,
    ) -> ChannelResult<Vec<InboundMessage>> {
        // Parse the Telegram Update JSON
        let update: TelegramUpdate = serde_json::from_slice(&body).map_err(|e| {
            tracing::warn!(error = %e, "Failed to parse Telegram update JSON");
            ChannelError::ReceiveFailed(format!("Invalid Telegram update JSON: {e}"))
        })?;

        let mut messages = Vec::new();

        // Handle message updates
        if let Some(msg) = update.message {
            messages.extend(self.process_message(msg).await?);
        }

        // Handle edited messages
        if let Some(msg) = update.edited_message {
            messages.extend(self.process_message(msg).await?);
        }

        // Handle callback queries
        if let Some(callback) = update.callback_query {
            if let Err(e) = self.process_callback_query(callback).await {
                tracing::warn!(error = %e, "Failed to process callback query");
            }
        }

        // Handle poll answers
        if let Some(poll_answer) = update.poll_answer {
            if let Some(inbound) = self.process_poll_answer(poll_answer) {
                messages.push(inbound);
            }
        }

        // Handle message reactions
        if let Some(reaction) = update.message_reaction {
            if let Some(inbound) = self.process_reaction(reaction) {
                messages.push(inbound);
            }
        }

        Ok(messages)
    }

    fn path(&self) -> &str {
        &self.path
    }
}

impl TelegramWebhookHandler {
    /// Process a Telegram Message into InboundMessage
    async fn process_message(
        &self,
        msg: TelegramMessage,
    ) -> ChannelResult<Vec<InboundMessage>> {
        // Extract user info
        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
        let chat_id = msg.chat.id.0;

        // Access control - check user and group allowlists
        let is_group = msg.chat.is_group() || msg.chat.is_supergroup();

        if is_group && !self.is_group_allowed(chat_id) {
            tracing::debug!(chat_id = %chat_id, "Ignoring message from disallowed group");
            return Ok(vec![]);
        }

        if !self.is_user_allowed(user_id) {
            tracing::debug!(user_id = %user_id, "Ignoring message from disallowed user");
            return Ok(vec![]);
        }

        // Extract text content
        let text = extract_message_text(&msg);

        // Skip empty messages
        if text.is_empty() {
            return Ok(vec![]);
        }

        // Build conversation ID with thread support
        let conversation_id = if let Some(thread_id) = msg.thread_id {
            ConversationId::new(format!("{}:topic:{}", chat_id, thread_id.0 .0))
        } else {
            ConversationId::new(chat_id.to_string())
        };

        // Extract sender info
        let (sender_id, sender_name) = if let Some(from) = &msg.from {
            (
                UserId::new(from.id.0.to_string()),
                Some(
                    from.username
                        .clone()
                        .unwrap_or_else(|| from.first_name.clone()),
                ),
            )
        } else {
            (UserId::new("unknown".to_string()), None)
        };

        // Extract attachments
        let attachments = extract_attachments_webhook(&msg);

        // Build metadata
        let mut metadata: Vec<MessageMeta> = Vec::new();

        // Media group ID
        if let Some(group_id) = msg.media_group_id {
            metadata.push(MessageMeta::MediaGroupId(group_id));
        }

        // Forward origin
        if let Some(forward) = msg.forward_origin {
            metadata.push(MessageMeta::ForwardOrigin {
                sender_name: forward.sender_name(),
                date: forward.date.map(|d| Utc.timestamp_opt(d, 0).single()).flatten(),
            });
        }

        // Timestamp
        let timestamp = Utc
            .timestamp_opt(msg.date, 0)
            .single()
            .unwrap_or_else(Utc::now);

        // Reply-to message
        let reply_to = msg.reply_to_message.as_ref().map(|r| MessageId::new(r.id.0.to_string()));

        let inbound = InboundMessage {
            id: MessageId::new(msg.id.0.to_string()),
            channel_id: self.channel_id.clone(),
            conversation_id,
            sender_id,
            sender_name,
            text,
            attachments,
            timestamp,
            reply_to,
            is_group,
            raw: Some(serde_json::to_value(&msg).unwrap_or_default()),
            metadata,
        };

        Ok(vec![inbound])
    }

    /// Process a callback query
    async fn process_callback_query(&self, query: TelegramCallbackQuery) -> Result<(), ChannelError> {
        let (raw_chat_id, thread_id_val) = if let Some(message) = &query.message {
            let chat = message.chat.id.0;
            let tid = message.thread_id.map(|t| t.0 .0);
            (chat, tid)
        } else {
            (0, None)
        };

        let conv_id_str = if let Some(tid) = thread_id_val {
            format!("{}:topic:{}", raw_chat_id, tid)
        } else {
            raw_chat_id.to_string()
        };

        let msg_id_str = query
            .message
            .as_ref()
            .map(|m| m.id.0.to_string())
            .unwrap_or_default();

        // Access check
        let is_group = raw_chat_id < 0;
        if is_group && !self.is_group_allowed(raw_chat_id) {
            return Ok(());
        }
        if !self.is_user_allowed(query.from.id.0 as i64) {
            return Ok(());
        }

        // Send callback query
        if let Some(data) = &query.data {
            let callback_query = CallbackQuery {
                id: query.id.clone(),
                user_id: UserId::new(query.from.id.to_string()),
                chat_id: ConversationId::new(conv_id_str.clone()),
                message_id: MessageId::new(msg_id_str),
                data: data.clone(),
            };

            if let Err(e) = self.callback_tx.send(callback_query).await {
                tracing::warn!("Failed to send callback query to channel: {}", e);
            }

            // Also send as inbound message for routing
            let inbound = InboundMessage {
                id: MessageId::new(format!("cb_{}", query.id)),
                channel_id: self.channel_id.clone(),
                conversation_id: ConversationId::new(conv_id_str),
                sender_id: UserId::new(query.from.id.to_string()),
                sender_name: query
                    .from
                    .username
                    .clone()
                    .or_else(|| Some(query.from.first_name.clone())),
                text: data.clone(),
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: None,
                is_group: raw_chat_id < 0,
                raw: None,
                metadata: vec![],
            };

            if let Err(e) = self.inbound_tx.send(inbound).await {
                tracing::warn!("Failed to send callback inbound message: {}", e);
            }
        }

        Ok(())
    }

    /// Process a poll answer
    fn process_poll_answer(&self, poll_answer: TelegramPollAnswer) -> Option<InboundMessage> {
        let (conversation_id, sender_id, sender_name) = match &poll_answer.user {
            Some(user) => (
                ConversationId::new(user.id.to_string()),
                UserId::new(user.id.to_string()),
                user.username.clone().or_else(|| Some(user.first_name.clone())),
            ),
            None => (
                ConversationId::new("anonymous".to_string()),
                UserId::new("unknown".to_string()),
                None,
            ),
        };

        Some(InboundMessage {
            id: MessageId::new(format!("poll_{}", poll_answer.poll_id)),
            channel_id: self.channel_id.clone(),
            conversation_id,
            sender_id,
            sender_name,
            text: format!("Poll answer: {:?}", poll_answer.option_ids),
            attachments: Vec::new(),
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![MessageMeta::PollAnswer {
                poll_id: poll_answer.poll_id,
                option_ids: poll_answer.option_ids,
            }],
        })
    }

    /// Process a message reaction update
    fn process_reaction(
        &self,
        reaction: TelegramMessageReactionUpdated,
    ) -> Option<InboundMessage> {
        // For now, create a simple notification
        // The actual reaction handling can be enhanced later
        Some(InboundMessage {
            id: MessageId::new(format!("reaction_{}", reaction.message_id.0)),
            channel_id: self.channel_id.clone(),
            conversation_id: ConversationId::new(reaction.chat.id.0.to_string()),
            sender_id: UserId::new("system".to_string()),
            sender_name: None,
            text: format!("Message reaction updated"),
            attachments: Vec::new(),
            timestamp: Utc::now(),
            reply_to: None,
            is_group: reaction.chat.is_group(),
            raw: None,
            metadata: vec![MessageMeta::Reaction {
                emojis: vec![], // Could extract from reaction if needed
            }],
        })
    }
}

// ── Telegram Update JSON types ─────────────────────────────────────────────────

/// Telegram Update object (partial - only fields we care about)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramUpdate {
    #[serde(default)]
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
    #[serde(default)]
    callback_query: Option<TelegramCallbackQuery>,
    #[serde(default)]
    poll_answer: Option<TelegramPollAnswer>,
    #[serde(default)]
    message_reaction: Option<TelegramMessageReactionUpdated>,
}

/// Telegram Message object (partial)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramMessage {
    message_id: teloxide::types::MessageId,
    #[serde(rename = "from")]
    from: Option<TelegramUser>,
    chat: TelegramChat,
    date: i64,
    #[serde(default)]
    thread_id: Option<TelegramThreadId>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    caption: Option<String>,
    #[serde(default)]
    media_group_id: Option<String>,
    #[serde(default)]
    forward_origin: Option<TelegramForwardOrigin>,
    #[serde(default)]
    reply_to_message: Option<Box<TelegramMessage>>,
    #[serde(default)]
    kind: TelegramMessageKind,
}

/// Simplified user type
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramUser {
    id: teloxide::types::UserId,
    username: Option<String>,
    first_name: String,
}

/// Chat type
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramChat {
    id: teloxide::types::ChatId,
    #[serde(rename = "type")]
    chat_type: String,
    is_group: Option<bool>,
    is_supergroup: Option<bool>,
}

impl TelegramChat {
    fn is_group(&self) -> bool {
        self.chat_type == "group"
            || self.chat_type == "supergroup"
            || self.is_group == Some(true)
            || self.is_supergroup == Some(true)
    }
}

/// Thread ID wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramThreadId {
    pub message_thread_id: i64,
}

impl TelegramThreadId {
    fn to_teloxide(&self) -> teloxide::types::MessageThreadId {
        teloxide::types::MessageThreadId(self.0)
    }
}

/// Forward origin (can be user, hidden user, or chat)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TelegramForwardOrigin {
    User {
        date: i64,
        sender_user: TelegramUser,
    },
    HiddenUser {
        date: i64,
        sender_user_name: String,
    },
    Chat {
        date: i64,
        sender_chat: TelegramChat,
        chat: TelegramChat,
    },
    Channel {
        date: i64,
        chat: TelegramChat,
    },
}

impl TelegramForwardOrigin {
    fn sender_name(&self) -> Option<String> {
        match self {
            TelegramForwardOrigin::User { sender_user, .. } => {
                Some(sender_user.username.clone().unwrap_or_else(|| sender_user.first_name.clone()))
            }
            TelegramForwardOrigin::HiddenUser {
                sender_user_name, ..
            } => Some(sender_user_name.clone()),
            TelegramForwardOrigin::Chat { sender_chat, .. } => sender_chat.title(),
            TelegramForwardOrigin::Channel { chat, .. } => chat.title(),
        }
    }

    fn date(&self) -> Option<i64> {
        match self {
            TelegramForwardOrigin::User { date, .. } => Some(*date),
            TelegramForwardOrigin::HiddenUser { date, .. } => Some(*date),
            TelegramForwardOrigin::Chat { date, .. } => Some(*date),
            TelegramForwardOrigin::Channel { date, .. } => Some(*date),
        }
    }
}

impl TelegramChat {
    fn title(&self) -> Option<String> {
        None // Chat struct doesn't have title field in our simplified version
    }
}

/// Message kind for media content
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(untagged)]
enum TelegramMessageKind {
    #[default]
    Empty,
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: Option<String>,
    },
    #[serde(rename = "photo")]
    Photo {
        #[serde(default)]
        caption: Option<String>,
    },
    #[serde(rename = "document")]
    Document {
        #[serde(default)]
        caption: Option<String>,
    },
    #[serde(rename = "video")]
    Video {
        #[serde(default)]
        caption: Option<String>,
    },
    #[serde(rename = "audio")]
    Audio {
        #[serde(default)]
        caption: Option<String>,
    },
    #[serde(rename = "voice")]
    Voice {
        #[serde(default)]
        caption: Option<String>,
    },
    #[serde(rename = "sticker")]
    Sticker {
        #[serde(default)]
        sticker: TelegramSticker,
    },
}

/// Simplified sticker type
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramSticker {
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    set_name: Option<String>,
}

/// Callback query from inline keyboard
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramCallbackQuery {
    id: String,
    from: TelegramUser,
    #[serde(default)]
    message: Option<TelegramCallbackMessage>,
    #[serde(default)]
    data: Option<String>,
}

/// Callback message (simplified)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramCallbackMessage {
    message_id: teloxide::types::MessageId,
    chat: TelegramChat,
    #[serde(default)]
    thread_id: Option<TelegramThreadId>,
}

/// Poll answer
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramPollAnswer {
    poll_id: String,
    #[serde(default)]
    user: Option<TelegramUser>,
    #[serde(default)]
    option_ids: Vec<u8>,
}

/// Message reaction update
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramMessageReactionUpdated {
    chat: TelegramChat,
    message_id: teloxide::types::MessageId,
    #[serde(default)]
    new_reaction: Option<Vec<TelegramReactionType>>,
}

/// Reaction type
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramReactionType {
    #[serde(rename = "type")]
    reaction_type: String,
    #[serde(default)]
    emoji: Option<String>,
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Extract text content from a Telegram message
fn extract_message_text(msg: &TelegramMessage) -> String {
    // Try text field first
    if let Some(text) = &msg.text {
        if !text.is_empty() {
            return text.clone();
        }
    }

    // Try caption
    if let Some(caption) = &msg.caption {
        if !caption.is_empty() {
            return caption.clone();
        }
    }

    // Try parsing kind for text
    match &msg.kind {
        TelegramMessageKind::Text { text } => text.clone().unwrap_or_default(),
        TelegramMessageKind::Photo { caption } => caption.clone().unwrap_or_default(),
        TelegramMessageKind::Document { caption } => caption.clone().unwrap_or_default(),
        TelegramMessageKind::Video { caption } => caption.clone().unwrap_or_default(),
        TelegramMessageKind::Audio { caption } => caption.clone().unwrap_or_default(),
        TelegramMessageKind::Voice { caption } => caption.clone().unwrap_or_default(),
        TelegramMessageKind::Sticker { sticker } => {
            let emoji = sticker.emoji.as_deref().unwrap_or("?");
            format!("[Sticker: {}]", emoji)
        }
        TelegramMessageKind::Empty => String::new(),
    }
}

/// Extract attachments from a Telegram message (webhook mode - no URL resolution)
fn extract_attachments_webhook(msg: &TelegramMessage) -> Vec<crate::gateway::channel::Attachment> {
    use crate::gateway::channel::Attachment;

    match &msg.kind {
        TelegramMessageKind::Photo { .. } => {
            // For webhook, we don't have the full file object
            // Return a placeholder - URL resolution can happen later
            vec![Attachment {
                id: "photo_placeholder".to_string(),
                mime_type: "image/jpeg".to_string(),
                filename: None,
                size: None,
                url: None,
                path: None,
                data: None,
            }]
        }
        TelegramMessageKind::Document { caption } => vec![Attachment {
            id: "document_placeholder".to_string(),
            mime_type: "application/octet-stream".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        }],
        TelegramMessageKind::Video { .. } => vec![Attachment {
            id: "video_placeholder".to_string(),
            mime_type: "video/mp4".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        }],
        TelegramMessageKind::Audio { .. } => vec![Attachment {
            id: "audio_placeholder".to_string(),
            mime_type: "audio/mpeg".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        }],
        TelegramMessageKind::Voice { .. } => vec![Attachment {
            id: "voice_placeholder".to_string(),
            mime_type: "audio/ogg".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        }],
        TelegramMessageKind::Sticker { sticker } => {
            let mime = if sticker.set_name.as_ref().map(|s| s.contains("animated")).unwrap_or(false)
            {
                "application/x-tgsticker".to_string()
            } else {
                "image/webp".to_string()
            };
            vec![Attachment {
                id: sticker.file_id.clone().unwrap_or_default(),
                mime_type: mime,
                filename: None,
                size: None,
                url: None,
                path: None,
                data: None,
            }]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_message_text_from_text_field() {
        let msg = TelegramMessage {
            message_id: teloxide::types::MessageId(123),
            from: None,
            chat: TelegramChat {
                id: teloxide::types::ChatId(456),
                chat_type: "private".to_string(),
                is_group: Some(false),
                is_supergroup: Some(false),
            },
            date: 0,
            thread_id: None,
            text: Some("Hello world".to_string()),
            caption: None,
            media_group_id: None,
            forward_origin: None,
            reply_to_message: None,
            kind: TelegramMessageKind::default(),
        };

        assert_eq!(extract_message_text(&msg), "Hello world");
    }

    #[test]
    fn test_extract_message_text_from_caption() {
        let msg = TelegramMessage {
            message_id: teloxide::types::MessageId(123),
            from: None,
            chat: TelegramChat {
                id: teloxide::types::ChatId(456),
                chat_type: "private".to_string(),
                is_group: Some(false),
                is_supergroup: Some(false),
            },
            date: 0,
            thread_id: None,
            text: None,
            caption: Some("Photo caption".to_string()),
            media_group_id: None,
            forward_origin: None,
            reply_to_message: None,
            kind: TelegramMessageKind::default(),
        };

        assert_eq!(extract_message_text(&msg), "Photo caption");
    }

    #[test]
    fn test_chat_is_group() {
        let private_chat = TelegramChat {
            id: teloxide::types::ChatId(123),
            chat_type: "private".to_string(),
            is_group: Some(false),
            is_supergroup: Some(false),
        };
        assert!(!private_chat.is_group());

        let group_chat = TelegramChat {
            id: teloxide::types::ChatId(-100123),
            chat_type: "group".to_string(),
            is_group: Some(true),
            is_supergroup: Some(false),
        };
        assert!(group_chat.is_group());

        let supergroup_chat = TelegramChat {
            id: teloxide::types::ChatId(-100123),
            chat_type: "supergroup".to_string(),
            is_group: Some(false),
            is_supergroup: Some(true),
        };
        assert!(supergroup_chat.is_group());
    }

    #[test]
    fn test_forward_origin_sender_name() {
        let user_origin = TelegramForwardOrigin::User {
            date: 123456,
            sender_user: TelegramUser {
                id: teloxide::types::UserId(123),
                username: Some("testuser".to_string()),
                first_name: "Test".to_string(),
            },
        };
        assert_eq!(user_origin.sender_name(), Some("testuser".to_string()));

        let hidden_origin = TelegramForwardOrigin::HiddenUser {
            date: 123456,
            sender_user_name: "Hidden Name".to_string(),
        };
        assert_eq!(hidden_origin.sender_name(), Some("Hidden Name".to_string()));
    }

    #[test]
    fn test_user_allowed_logic() {
        let handler = TelegramWebhookHandler::new(
            "bot_token".to_string(),
            ChannelId::new("test"),
            "/webhook/telegram".to_string(),
            Some("secret".to_string()),
            mpsc::channel(100).0,
            mpsc::channel(100).0,
            vec![1, 2, 3], // allowed users
            vec![],
        );

        assert!(handler.is_user_allowed(1));
        assert!(handler.is_user_allowed(2));
        assert!(!handler.is_user_allowed(99));

        // Empty allowlist = allow all
        let open_handler = TelegramWebhookHandler::new(
            "bot_token".to_string(),
            ChannelId::new("test"),
            "/webhook/telegram".to_string(),
            Some("secret".to_string()),
            mpsc::channel(100).0,
            mpsc::channel(100).0,
            vec![], // empty = allow all
            vec![],
        );

        assert!(open_handler.is_user_allowed(999));
    }

    #[test]
    fn test_webhook_handler_verify() {
        use axum::http::HeaderMap;

        let handler = TelegramWebhookHandler::new(
            "bot_token".to_string(),
            ChannelId::new("test"),
            "/webhook/telegram".to_string(),
            Some("my_secret_token".to_string()),
            mpsc::channel(100).0,
            mpsc::channel(100).0,
            vec![],
            vec![],
        );

        // No secret configured - should pass
        let no_secret_handler = TelegramWebhookHandler::new(
            "bot_token".to_string(),
            ChannelId::new("test"),
            "/webhook/telegram".to_string(),
            None,
            mpsc::channel(100).0,
            mpsc::channel(100).0,
            vec![],
            vec![],
        );
        let mut headers = HeaderMap::new();
        assert!(no_secret_handler.verify(&headers, &[]));

        // Wrong secret
        let mut headers = HeaderMap::new();
        headers.insert(
            TELEGRAM_SECRET_TOKEN_HEADER,
            "wrong_token".parse().unwrap(),
        );
        assert!(!handler.verify(&headers, &[]));

        // Correct secret
        let mut headers = HeaderMap::new();
        headers.insert(
            TELEGRAM_SECRET_TOKEN_HEADER,
            "my_secret_token".parse().unwrap(),
        );
        assert!(handler.verify(&headers, &[]));
    }

    #[tokio::test]
    async fn test_webhook_handler_handle_update() {
        use axum::http::HeaderMap;

        let (inbound_tx, mut inbound_rx) = mpsc::channel(100);
        let (callback_tx, _callback_rx) = mpsc::channel(100);

        let handler = TelegramWebhookHandler::new(
            "bot_token".to_string(),
            ChannelId::new("telegram"),
            "/webhook/telegram".to_string(),
            None,
            inbound_tx,
            callback_tx,
            vec![], // allow all users
            vec![],
        );

        // Create a simple message update
        let update_json = serde_json::json!({
            "update_id": 123456789,
            "message": {
                "message_id": 1,
                "from": {
                    "id": 111,
                    "is_bot": false,
                    "first_name": "Test",
                    "username": "testuser"
                },
                "chat": {
                    "id": 111,
                    "type": "private"
                },
                "date": 1234567890,
                "text": "Hello from webhook!"
            }
        });

        let body = serde_json::to_vec(&update_json).unwrap();
        let headers = HeaderMap::new();

        let result = handler.handle(&headers, bytes::Bytes::from(body)).await;
        assert!(result.is_ok());

        let messages = result.unwrap();
        assert!(!messages.is_empty());

        let msg = messages.first().unwrap();
        assert_eq!(msg.text, "Hello from webhook!");
        assert_eq!(msg.sender_id.as_str(), "111");
    }
}
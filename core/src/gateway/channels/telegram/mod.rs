//! Telegram Channel Implementation
//!
//! Integrates with the Telegram Bot API using the teloxide framework.
//!
//! # Features
//!
//! - Long-polling or webhook mode
//! - User/group allowlists
//! - File and image attachments
//! - Inline keyboards (future)
//! - Reply threading
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "telegram"
//! channel_type = "telegram"
//! enabled = true
//!
//! [channels.config]
//! bot_token = "123456:ABC..."
//! allowed_users = [12345, 67890]
//! ```

pub mod config;
pub mod message_ops;

pub use config::{TelegramConfig, WebhookConfig};
pub use message_ops::TelegramMessageOps;

use crate::gateway::channel::{
    Attachment, CallbackQuery, Channel, ChannelCapabilities, ChannelError, ChannelFactory,
    ChannelId, ChannelInfo, ChannelResult, ChannelStatus, ConversationId, InboundMessage,
    InlineKeyboard, MessageId, OutboundMessage, SendResult, UserId,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

#[cfg(feature = "telegram")]
use teloxide::{
    prelude::*,
    types::{
        CallbackQuery as TgCallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup,
        InputFile, MediaKind, MessageKind, ParseMode,
    },
};

/// Telegram channel implementation
pub struct TelegramChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: TelegramConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Callback query sender
    callback_tx: mpsc::Sender<CallbackQuery>,
    /// Callback query receiver (taken on first call)
    callback_rx: Option<mpsc::Receiver<CallbackQuery>>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// Teloxide bot instance
    #[cfg(feature = "telegram")]
    bot: Option<Bot>,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(id: impl Into<String>, config: TelegramConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);
        let (callback_tx, callback_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Telegram".to_string(),
            channel_type: "telegram".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            callback_tx,
            callback_rx: Some(callback_rx),
            shutdown_tx: None,
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            #[cfg(feature = "telegram")]
            bot: None,
        }
    }

    /// Get Telegram-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: false, // Telegram reactions are limited
            replies: true,
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true, // Markdown/HTML support
            max_message_length: 4096,
            max_attachment_size: 50 * 1024 * 1024, // 50MB
        }
    }

    /// Convert Telegram message to InboundMessage
    #[cfg(feature = "telegram")]
    fn convert_message(msg: &teloxide::types::Message, config: &TelegramConfig) -> Option<InboundMessage> {
        // Get sender info
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
            (UserId::new("unknown"), None)
        };

        // Check if user is allowed
        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
        if !config.is_user_allowed(user_id) {
            tracing::debug!("User {} not in allowlist, ignoring message", user_id);
            return None;
        }

        // Check if group is allowed
        let is_group = msg.chat.is_group() || msg.chat.is_supergroup();
        if is_group && !config.is_group_allowed(msg.chat.id.0) {
            tracing::debug!("Group {} not in allowlist, ignoring message", msg.chat.id.0);
            return None;
        }

        // Extract text content
        let text = match &msg.kind {
            MessageKind::Common(common) => match &common.media_kind {
                MediaKind::Text(text_msg) => text_msg.text.clone(),
                MediaKind::Photo(photo) => photo.caption.clone().unwrap_or_default(),
                MediaKind::Document(doc) => doc.caption.clone().unwrap_or_default(),
                MediaKind::Audio(audio) => audio.caption.clone().unwrap_or_default(),
                MediaKind::Video(video) => video.caption.clone().unwrap_or_default(),
                MediaKind::Voice(voice) => voice.caption.clone().unwrap_or_default(),
                _ => String::new(),
            },
            _ => return None, // Ignore non-common messages (service messages, etc.)
        };

        // Skip empty messages
        if text.is_empty() {
            return None;
        }

        // Extract attachments
        let attachments = Self::extract_attachments(msg);

        // Get reply-to message ID
        let reply_to = msg
            .reply_to_message()
            .map(|r| MessageId::new(r.id.0.to_string()));

        // Convert timestamp
        let timestamp = Utc
            .timestamp_opt(msg.date.timestamp(), 0)
            .single()
            .unwrap_or_else(Utc::now);

        Some(InboundMessage {
            id: MessageId::new(msg.id.0.to_string()),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new(msg.chat.id.0.to_string()),
            sender_id,
            sender_name,
            text,
            attachments,
            timestamp,
            reply_to,
            is_group,
            raw: Some(serde_json::to_value(msg).unwrap_or_default()),
        })
    }

    /// Extract attachments from Telegram message
    #[cfg(feature = "telegram")]
    fn extract_attachments(msg: &teloxide::types::Message) -> Vec<Attachment> {
        let mut attachments = Vec::new();

        if let MessageKind::Common(common) = &msg.kind {
            match &common.media_kind {
                MediaKind::Photo(photo) => {
                    // Get the largest photo size
                    if let Some(largest) = photo.photo.last() {
                        attachments.push(Attachment {
                            id: largest.file.id.clone(),
                            mime_type: "image/jpeg".to_string(),
                            filename: None,
                            size: Some(largest.file.size as u64),
                            url: None, // Will be fetched via getFile API
                            path: None,
                            data: None,
                        });
                    }
                }
                MediaKind::Document(doc) => {
                    attachments.push(Attachment {
                        id: doc.document.file.id.clone(),
                        mime_type: doc
                            .document
                            .mime_type
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "application/octet-stream".to_string()),
                        filename: doc.document.file_name.clone(),
                        size: Some(doc.document.file.size as u64),
                        url: None,
                        path: None,
                        data: None,
                    });
                }
                MediaKind::Audio(audio) => {
                    attachments.push(Attachment {
                        id: audio.audio.file.id.clone(),
                        mime_type: audio
                            .audio
                            .mime_type
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "audio/mpeg".to_string()),
                        filename: audio.audio.file_name.clone(),
                        size: Some(audio.audio.file.size as u64),
                        url: None,
                        path: None,
                        data: None,
                    });
                }
                MediaKind::Video(video) => {
                    attachments.push(Attachment {
                        id: video.video.file.id.clone(),
                        mime_type: video
                            .video
                            .mime_type
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "video/mp4".to_string()),
                        filename: video.video.file_name.clone(),
                        size: Some(video.video.file.size as u64),
                        url: None,
                        path: None,
                        data: None,
                    });
                }
                MediaKind::Voice(voice) => {
                    attachments.push(Attachment {
                        id: voice.voice.file.id.clone(),
                        mime_type: voice
                            .voice
                            .mime_type
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "audio/ogg".to_string()),
                        filename: None,
                        size: Some(voice.voice.file.size as u64),
                        url: None,
                        path: None,
                        data: None,
                    });
                }
                _ => {}
            }
        }

        attachments
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn status(&self) -> ChannelStatus {
        // Return cached status synchronously
        // The actual status is updated by the polling/webhook task
        self.info.status
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config
            .validate()
            .map_err(|e| ChannelError::ConfigError(e))?;

        #[cfg(feature = "telegram")]
        {
            self.set_status(ChannelStatus::Connecting).await;
            tracing::info!("Starting Telegram channel...");

            // Create bot instance
            let bot = Bot::new(&self.config.bot_token);

            // Verify bot token by getting bot info
            match bot.get_me().await {
                Ok(me) => {
                    tracing::info!(
                        "Telegram bot connected: @{} ({})",
                        me.username(),
                        me.id
                    );
                }
                Err(e) => {
                    self.set_status(ChannelStatus::Error).await;
                    return Err(ChannelError::AuthFailed(format!(
                        "Failed to verify bot token: {}",
                        e
                    )));
                }
            }

            // Store bot instance
            self.bot = Some(bot.clone());

            // Create shutdown channel
            let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
            self.shutdown_tx = Some(shutdown_tx);

            // Start message polling
            let inbound_tx = self.inbound_tx.clone();
            let callback_tx = self.callback_tx.clone();
            let config = self.config.clone();
            let status = self.status.clone();

            tokio::spawn(async move {
                tracing::info!("Starting Telegram long-polling...");
                *status.write().await = ChannelStatus::Connected;

                // Message handler
                let message_handler = Update::filter_message().endpoint(
                    move |_bot: Bot, msg: teloxide::types::Message| {
                        let inbound_tx = inbound_tx.clone();
                        let config = config.clone();
                        async move {
                            if let Some(inbound) = TelegramChannel::convert_message(&msg, &config) {
                                if let Err(e) = inbound_tx.send(inbound).await {
                                    tracing::error!("Failed to send inbound message: {}", e);
                                }
                            }
                            Ok::<(), std::convert::Infallible>(())
                        }
                    },
                );

                // Callback query handler
                let callback_handler = Update::filter_callback_query().endpoint(
                    move |bot: Bot, q: TgCallbackQuery| {
                        let tx = callback_tx.clone();
                        async move {
                            if let Some(data) = q.data.clone() {
                                let chat_id = q
                                    .message
                                    .as_ref()
                                    .map(|m| m.chat().id.to_string())
                                    .unwrap_or_default();
                                let msg_id = q
                                    .message
                                    .as_ref()
                                    .map(|m| m.id().to_string())
                                    .unwrap_or_default();

                                let query = CallbackQuery {
                                    id: q.id.clone(),
                                    user_id: UserId::new(q.from.id.to_string()),
                                    chat_id: ConversationId::new(chat_id),
                                    message_id: MessageId::new(msg_id),
                                    data,
                                };

                                if let Err(e) = tx.send(query).await {
                                    tracing::error!("Failed to send callback query: {}", e);
                                }
                            }

                            // Answer callback to remove loading indicator
                            if let Err(e) = bot.answer_callback_query(&q.id).await {
                                tracing::warn!("Failed to answer callback query: {}", e);
                            }

                            Ok::<(), std::convert::Infallible>(())
                        }
                    },
                );

                // Combine handlers
                let handler = dptree::entry()
                    .branch(message_handler)
                    .branch(callback_handler);

                let mut dispatcher = Dispatcher::builder(bot, handler)
                    .enable_ctrlc_handler()
                    .build();

                tokio::select! {
                    _ = dispatcher.dispatch() => {
                        tracing::info!("Telegram dispatcher stopped");
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Telegram channel shutdown requested");
                        // Dispatcher will stop when this task ends
                    }
                }

                *status.write().await = ChannelStatus::Disconnected;
            });

            self.set_status(ChannelStatus::Connected).await;
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            Err(ChannelError::UnsupportedFeature(
                "Telegram support not compiled (enable 'telegram' feature)".to_string(),
            ))
        }
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Telegram channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.set_status(ChannelStatus::Disconnected).await;

        #[cfg(feature = "telegram")]
        {
            self.bot = None;
        }

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        #[cfg(feature = "telegram")]
        {
            let bot = self
                .bot
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

            let chat_id = ChatId(
                message
                    .conversation_id
                    .as_str()
                    .parse::<i64>()
                    .map_err(|e| ChannelError::SendFailed(format!("Invalid chat ID: {}", e)))?,
            );

            // Send typing indicator if enabled
            if self.config.send_typing {
                let _ = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing).await;
            }

            // Build and send message
            let mut request = bot.send_message(chat_id, &message.text);

            // Set parse mode for Markdown support
            request = request.parse_mode(ParseMode::MarkdownV2);

            // Set reply-to if specified
            if let Some(reply_to) = &message.reply_to {
                if let Ok(msg_id) = reply_to.as_str().parse::<i32>() {
                    request = request.reply_parameters(teloxide::types::ReplyParameters::new(
                        teloxide::types::MessageId(msg_id),
                    ));
                }
            }

            // Add inline keyboard if present
            if let Some(ref keyboard) = message.inline_keyboard {
                let markup = InlineKeyboardMarkup::new(
                    keyboard.rows.iter().map(|row| {
                        row.iter().map(|btn| {
                            InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
                        }).collect::<Vec<_>>()
                    }).collect::<Vec<_>>()
                );
                request = request.reply_markup(markup);
            }

            // Send the message
            let sent = request
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Telegram send error: {}", e)))?;

            // Send attachments if any
            for attachment in &message.attachments {
                self.send_attachment(bot, chat_id, attachment).await?;
            }

            Ok(SendResult {
                message_id: MessageId::new(sent.id.0.to_string()),
                timestamp: Utc::now(),
            })
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message;
            Err(ChannelError::UnsupportedFeature(
                "Telegram support not compiled".to_string(),
            ))
        }
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = self
                .bot
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

            let chat_id = ChatId(
                conversation_id
                    .as_str()
                    .parse::<i64>()
                    .map_err(|e| ChannelError::Internal(format!("Invalid chat ID: {}", e)))?,
            );

            bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing)
                .await
                .map_err(|e| ChannelError::Internal(format!("Failed to send typing: {}", e)))?;

            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = conversation_id;
            Err(ChannelError::UnsupportedFeature(
                "Telegram support not compiled".to_string(),
            ))
        }
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        #[cfg(feature = "telegram")]
        {
            // Note: Editing requires both message_id and chat_id
            // This is a limitation - we'd need to track chat_id per message
            let _ = (message_id, new_text);
            Err(ChannelError::UnsupportedFeature(
                "Message editing requires chat context".to_string(),
            ))
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = (message_id, new_text);
            Err(ChannelError::UnsupportedFeature(
                "Telegram support not compiled".to_string(),
            ))
        }
    }

    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        #[cfg(feature = "telegram")]
        {
            // Note: Deleting requires both message_id and chat_id
            let _ = message_id;
            Err(ChannelError::UnsupportedFeature(
                "Message deletion requires chat context".to_string(),
            ))
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message_id;
            Err(ChannelError::UnsupportedFeature(
                "Telegram support not compiled".to_string(),
            ))
        }
    }
}

impl TelegramChannel {
    /// Take the inbound receiver (can only be called once)
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.take()
    }

    /// Take the callback receiver (can only be called once)
    pub fn take_callback_receiver(&mut self) -> Option<mpsc::Receiver<CallbackQuery>> {
        self.callback_rx.take()
    }

    /// Send an attachment
    #[cfg(feature = "telegram")]
    async fn send_attachment(
        &self,
        bot: &Bot,
        chat_id: ChatId,
        attachment: &Attachment,
    ) -> ChannelResult<()> {
        let input_file = if let Some(data) = &attachment.data {
            InputFile::memory(data.clone())
        } else if let Some(path) = &attachment.path {
            InputFile::file(path)
        } else if let Some(url) = &attachment.url {
            InputFile::url(url.parse().map_err(|e| {
                ChannelError::SendFailed(format!("Invalid attachment URL: {}", e))
            })?)
        } else {
            return Err(ChannelError::SendFailed(
                "Attachment has no data, path, or URL".to_string(),
            ));
        };

        // Determine attachment type by MIME type
        let mime = &attachment.mime_type;
        if mime.starts_with("image/") {
            bot.send_photo(chat_id, input_file)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send photo: {}", e)))?;
        } else if mime.starts_with("audio/") {
            bot.send_audio(chat_id, input_file)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send audio: {}", e)))?;
        } else if mime.starts_with("video/") {
            bot.send_video(chat_id, input_file)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send video: {}", e)))?;
        } else {
            bot.send_document(chat_id, input_file)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send document: {}", e)))?;
        }

        Ok(())
    }

    /// Edit a message's text and/or inline keyboard
    ///
    /// # Arguments
    /// * `chat_id` - The chat containing the message
    /// * `message_id` - The message to edit
    /// * `new_text` - Optional new text (if None, text is not changed)
    /// * `keyboard` - Optional new keyboard (if None, keyboard is removed)
    #[cfg(feature = "telegram")]
    pub async fn edit_message(
        &self,
        chat_id: &ConversationId,
        message_id: &MessageId,
        new_text: Option<&str>,
        keyboard: Option<&InlineKeyboard>,
    ) -> ChannelResult<()> {
        let bot = self
            .bot
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

        let chat = ChatId(chat_id.as_str().parse().map_err(|_| {
            ChannelError::SendFailed(format!("Invalid chat ID: {}", chat_id.as_str()))
        })?);

        let msg_id = teloxide::types::MessageId(message_id.as_str().parse().map_err(|_| {
            ChannelError::SendFailed("Invalid message ID".into())
        })?);

        if let Some(text) = new_text {
            // Edit text (and optionally keyboard)
            let mut request = bot.edit_message_text(chat, msg_id, text);

            // Set keyboard or remove it
            if let Some(kb) = keyboard {
                let markup = InlineKeyboardMarkup::new(
                    kb.rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|btn| {
                                    InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>(),
                );
                request = request.reply_markup(markup);
            } else {
                // Remove keyboard by setting empty markup
                request = request.reply_markup(InlineKeyboardMarkup::default());
            }

            request
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        } else if let Some(kb) = keyboard {
            // Edit only the keyboard (need to use edit_message_reply_markup)
            let markup = InlineKeyboardMarkup::new(
                kb.rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|btn| {
                                InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
            );

            bot.edit_message_reply_markup(chat, msg_id)
                .reply_markup(markup)
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        } else {
            // Remove keyboard only
            bot.edit_message_reply_markup(chat, msg_id)
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        }

        Ok(())
    }

    /// Edit a message's text and/or inline keyboard (stub for non-telegram builds)
    #[cfg(not(feature = "telegram"))]
    pub async fn edit_message(
        &self,
        _chat_id: &ConversationId,
        _message_id: &MessageId,
        _new_text: Option<&str>,
        _keyboard: Option<&InlineKeyboard>,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "Telegram support not compiled".to_string(),
        ))
    }
}

/// Factory for creating Telegram channels
pub struct TelegramChannelFactory;

#[async_trait]
impl ChannelFactory for TelegramChannelFactory {
    fn channel_type(&self) -> &str {
        "telegram"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: TelegramConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Telegram config: {}", e)))?;

        config
            .validate()
            .map_err(|e| ChannelError::ConfigError(e))?;

        Ok(Box::new(TelegramChannel::new("telegram", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = TelegramChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.replies);
        assert_eq!(caps.max_message_length, 4096);
    }

    #[test]
    fn test_channel_creation() {
        let config = TelegramConfig {
            bot_token: "123:ABC".to_string(),
            ..Default::default()
        };
        let channel = TelegramChannel::new("telegram-test", config);
        assert_eq!(channel.info().id.as_str(), "telegram-test");
        assert_eq!(channel.info().channel_type, "telegram");
    }
}

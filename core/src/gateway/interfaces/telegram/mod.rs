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
pub mod group_chat;
pub mod message_ops;

pub use config::{TelegramConfig, WebhookConfig};
pub use message_ops::TelegramMessageOps;

use crate::gateway::channel::{
    Attachment, CallbackQuery, Channel, ChannelCapabilities, ChannelError, ChannelFactory,
    ChannelId, ChannelInfo, ChannelResult, ChannelState, ChannelStatus, ConversationId,
    InboundMessage, InlineKeyboard, MessageId, OutboundMessage, SendResult, UserId,
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tokio::sync::{mpsc, oneshot};

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
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Callback query sender
    callback_tx: mpsc::Sender<CallbackQuery>,
    /// Callback query receiver (taken on first call)
    callback_rx: Option<mpsc::Receiver<CallbackQuery>>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Teloxide bot instance
    bot: Option<Bot>,
    /// Slash commands to register with Telegram Bot API (command, description)
    slash_commands: Vec<(String, String)>,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(id: impl Into<String>, config: TelegramConfig) -> Self {
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
            channel_state: ChannelState::new(100),
            callback_tx,
            callback_rx: Some(callback_rx),
            shutdown_tx: None,
            bot: None,
            slash_commands: Vec::new(),
        }
    }

    /// Set slash commands to register with Telegram Bot API on startup.
    ///
    /// Each entry is (command_name, description). These are registered via
    /// `set_my_commands` so users see a command menu when typing `/`.
    pub fn with_slash_commands(mut self, commands: Vec<(String, String)>) -> Self {
        self.slash_commands = commands;
        self
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
        self.channel_state.set_status(status).await;
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config
            .validate()
            .map_err(ChannelError::ConfigError)?;

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

        // Register slash commands with Telegram Bot API
        // This makes commands appear in the menu when users type "/"
        if !self.slash_commands.is_empty() {
            use teloxide::types::BotCommand;

            // Telegram limits: max 100 commands, command name max 32 chars,
            // lowercase a-z, 0-9, underscore only
            let bot_commands: Vec<BotCommand> = self.slash_commands.iter()
                .take(100) // Telegram hard limit
                .filter_map(|(name, desc)| {
                    // Normalize: lowercase, replace hyphens with underscores, strip invalid chars
                    let normalized: String = name.to_lowercase().chars()
                        .map(|c| if c == '-' { '_' } else { c })
                        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                        .take(32) // Max command name length
                        .collect();
                    if normalized.is_empty() {
                        return None;
                    }
                    // Telegram description max 256 chars
                    let desc_truncated = if desc.chars().count() > 256 {
                        let truncated: String = desc.chars().take(253).collect();
                        format!("{}...", truncated)
                    } else {
                        desc.clone()
                    };
                    Some(BotCommand::new(normalized, desc_truncated))
                })
                .collect();

            if !bot_commands.is_empty() {
                // Clear old commands first, then set new ones (OpenClaw pattern)
                let _ = bot.delete_my_commands().await;
                match bot.set_my_commands(bot_commands.clone()).await {
                    Ok(_) => {
                        tracing::info!(
                            "Registered {} slash commands with Telegram Bot API",
                            bot_commands.len()
                        );
                    }
                    Err(e) => {
                        // Non-fatal: bot still works, just no command menu
                        tracing::warn!(
                            "Failed to register Telegram slash commands: {} (bot will still work)",
                            e
                        );
                    }
                }
            }
        }

        // Store bot instance
        self.bot = Some(bot.clone());

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        // Start message polling
        let inbound_tx = self.channel_state.sender();
        let callback_tx = self.callback_tx.clone();
        let config = self.config.clone();
        let status = self.channel_state.status_handle();

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

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Telegram channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.set_status(ChannelStatus::Disconnected).await;

        self.bot = None;

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
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

        // Helper to build a SendMessage request with optional reply-to and inline keyboard
        let build_request = |parse_mode: Option<ParseMode>| {
            let mut req = bot.send_message(chat_id, &message.text);
            if let Some(mode) = parse_mode {
                req = req.parse_mode(mode);
            }
            if let Some(reply_to) = &message.reply_to {
                if let Ok(msg_id) = reply_to.as_str().parse::<i32>() {
                    req = req.reply_parameters(teloxide::types::ReplyParameters::new(
                        teloxide::types::MessageId(msg_id),
                    ));
                }
            }
            if let Some(ref keyboard) = message.inline_keyboard {
                let markup = InlineKeyboardMarkup::new(
                    keyboard.rows.iter().map(|row| {
                        row.iter().map(|btn| {
                            InlineKeyboardButton::callback(&btn.text, &btn.callback_data)
                        }).collect::<Vec<_>>()
                    }).collect::<Vec<_>>()
                );
                req = req.reply_markup(markup);
            }
            req
        };

        // Try MarkdownV2 first, fall back to plain text if parsing fails
        let sent = match build_request(Some(ParseMode::MarkdownV2)).await {
            Ok(msg) => msg,
            Err(md_err) => {
                tracing::warn!(
                    "MarkdownV2 send failed, retrying as plain text: {}",
                    md_err
                );
                build_request(None)
                    .await
                    .map_err(|e| ChannelError::SendFailed(format!("Telegram send error: {}", e)))?
            }
        };

        // Send attachments if any
        for attachment in &message.attachments {
            self.send_attachment(bot, chat_id, attachment).await?;
        }

        Ok(SendResult {
            message_id: MessageId::new(sent.id.0.to_string()),
            timestamp: Utc::now(),
        })
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
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

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        // Note: Editing requires both message_id and chat_id
        // This is a limitation - we'd need to track chat_id per message
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "Message editing requires chat context".to_string(),
        ))
    }

    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        // Note: Deleting requires both message_id and chat_id
        let _ = message_id;
        Err(ChannelError::UnsupportedFeature(
            "Message deletion requires chat context".to_string(),
        ))
    }
}

impl TelegramChannel {
    /// Take the callback receiver (can only be called once)
    pub fn take_callback_receiver(&mut self) -> Option<mpsc::Receiver<CallbackQuery>> {
        self.callback_rx.take()
    }

    /// Send an attachment
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
            .map_err(ChannelError::ConfigError)?;

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

//! Discord Channel Implementation
//!
//! Integrates with the Discord API using the serenity framework.
//!
//! # Features
//!
//! - Guild and DM message handling
//! - Slash commands support
//! - Message embeds
//! - File attachments
//! - Typing indicators
//! - Reply threading
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "discord"
//! channel_type = "discord"
//! enabled = true
//!
//! [channels.config]
//! bot_token = "MTIzNDU2..."
//! allowed_guilds = [123456789]
//! dm_allowed = true
//! ```

pub mod config;
pub mod message_ops;

pub use config::{DiscordConfig, IntentsConfig};
pub use message_ops::DiscordMessageOps;

use crate::gateway::channel::{
    Attachment, Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId,
    ChannelInfo, ChannelResult, ChannelStatus, ConversationId, InboundMessage, MessageId,
    OutboundMessage, SendResult, UserId,
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

#[cfg(feature = "discord")]
use serenity::{
    all::{
        ChannelId as SerenityChannelId, Context, CreateAttachment, CreateMessage,
        EventHandler, GatewayIntents, Message, Ready,
    },
    Client,
};

/// Discord channel implementation
pub struct DiscordChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: DiscordConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// HTTP client for sending messages (serenity's Http)
    #[cfg(feature = "discord")]
    http: Option<Arc<serenity::http::Http>>,
}

impl DiscordChannel {
    /// Create a new Discord channel
    pub fn new(id: impl Into<String>, config: DiscordConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Discord".to_string(),
            channel_type: "discord".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            shutdown_tx: None,
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            #[cfg(feature = "discord")]
            http: None,
        }
    }

    /// Get Discord-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true, // Markdown support
            max_message_length: 2000,
            max_attachment_size: 25 * 1024 * 1024, // 25MB for normal, 100MB for Nitro
        }
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    /// Take the inbound receiver (can only be called once)
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.take()
    }
}

/// Event handler for Discord gateway events
#[cfg(feature = "discord")]
struct Handler {
    inbound_tx: mpsc::Sender<InboundMessage>,
    config: DiscordConfig,
    status: Arc<RwLock<ChannelStatus>>,
    bot_user_id: Arc<RwLock<Option<u64>>>,
}

#[cfg(feature = "discord")]
#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(
            "Discord bot connected: {}#{} ({})",
            ready.user.name,
            ready.user.discriminator.map(|d| d.to_string()).unwrap_or_default(),
            ready.user.id
        );

        // Store bot user ID for mention detection
        *self.bot_user_id.write().await = Some(ready.user.id.get());
        *self.status.write().await = ChannelStatus::Connected;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from bots (including self)
        if msg.author.bot {
            return;
        }

        // Check if this is a DM
        let is_dm = msg.guild_id.is_none();

        // Check DM permission
        if is_dm && !self.config.dm_allowed {
            tracing::debug!("DM from {} ignored (DMs disabled)", msg.author.id);
            return;
        }

        // Check guild permission
        if let Some(guild_id) = msg.guild_id {
            if !self.config.is_guild_allowed(guild_id.get()) {
                tracing::debug!("Message from guild {} ignored (not in allowlist)", guild_id);
                return;
            }
        }

        // Check channel permission
        if !self.config.is_channel_allowed(msg.channel_id.get()) {
            tracing::debug!(
                "Message from channel {} ignored (not in allowlist)",
                msg.channel_id
            );
            return;
        }

        // Check if bot was mentioned or if using prefix
        let bot_user_id = self.bot_user_id.read().await;
        let mentioned = bot_user_id
            .map(|id| msg.mentions.iter().any(|u| u.id.get() == id))
            .unwrap_or(false);

        let has_prefix = msg.content.starts_with(&self.config.command_prefix);

        // Only process if mentioned or has prefix (for guilds)
        // Always process DMs
        if !is_dm && !mentioned && !has_prefix && self.config.respond_to_mentions {
            return;
        }

        // Extract text (remove mention/prefix if present)
        let text = if has_prefix {
            msg.content[self.config.command_prefix.len()..].trim().to_string()
        } else if mentioned {
            // Remove the mention from the text
            let mention_pattern = format!("<@{}>", bot_user_id.unwrap_or(0));
            let mention_pattern_nick = format!("<@!{}>", bot_user_id.unwrap_or(0));
            msg.content
                .replace(&mention_pattern, "")
                .replace(&mention_pattern_nick, "")
                .trim()
                .to_string()
        } else {
            msg.content.clone()
        };

        // Skip empty messages
        if text.is_empty() && msg.attachments.is_empty() {
            return;
        }

        // Extract attachments
        let attachments: Vec<Attachment> = msg
            .attachments
            .iter()
            .map(|a| Attachment {
                id: a.id.to_string(),
                mime_type: a
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                filename: Some(a.filename.clone()),
                size: Some(a.size as u64),
                url: Some(a.url.clone()),
                path: None,
                data: None,
            })
            .collect();

        // Get reply-to message ID
        let reply_to = msg
            .referenced_message
            .as_ref()
            .map(|r| MessageId::new(r.id.to_string()));

        // Build conversation ID (channel ID for guilds, user ID for DMs)
        let conversation_id = if is_dm {
            ConversationId::new(format!("dm:{}", msg.author.id))
        } else {
            ConversationId::new(msg.channel_id.to_string())
        };

        // Create inbound message
        let inbound = InboundMessage {
            id: MessageId::new(msg.id.to_string()),
            channel_id: ChannelId::new("discord"),
            conversation_id,
            sender_id: UserId::new(msg.author.id.to_string()),
            sender_name: Some(msg.author.name.clone()),
            text,
            attachments,
            timestamp: Utc
                .timestamp_opt(msg.timestamp.unix_timestamp(), 0)
                .single()
                .unwrap_or_else(Utc::now),
            reply_to,
            is_group: !is_dm,
            raw: Some(serde_json::json!({
                "guild_id": msg.guild_id.map(|g| g.to_string()),
                "channel_id": msg.channel_id.to_string(),
            })),
        };

        // Send to channel
        if let Err(e) = self.inbound_tx.send(inbound).await {
            tracing::error!("Failed to send inbound Discord message: {}", e);
        }

        // Send typing indicator if enabled
        if self.config.send_typing {
            let _ = msg.channel_id.broadcast_typing(&ctx.http).await;
        }
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn status(&self) -> ChannelStatus {
        self.info.status
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        #[cfg(feature = "discord")]
        {
            self.set_status(ChannelStatus::Connecting).await;
            tracing::info!("Starting Discord channel...");

            // Build gateway intents
            let mut intents = GatewayIntents::empty();
            if self.config.intents.guild_messages {
                intents |= GatewayIntents::GUILD_MESSAGES;
            }
            if self.config.intents.direct_messages {
                intents |= GatewayIntents::DIRECT_MESSAGES;
            }
            if self.config.intents.message_content {
                intents |= GatewayIntents::MESSAGE_CONTENT;
            }
            if self.config.intents.guild_members {
                intents |= GatewayIntents::GUILD_MEMBERS;
            }

            // Create event handler
            let handler = Handler {
                inbound_tx: self.inbound_tx.clone(),
                config: self.config.clone(),
                status: self.status.clone(),
                bot_user_id: Arc::new(RwLock::new(None)),
            };

            // Build client
            let mut client = Client::builder(&self.config.bot_token, intents)
                .event_handler(handler)
                .await
                .map_err(|e| ChannelError::ConfigError(format!("Failed to create Discord client: {}", e)))?;

            // Store HTTP client for sending messages
            self.http = Some(client.http.clone());

            // Create shutdown channel
            let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
            self.shutdown_tx = Some(shutdown_tx);

            let status = self.status.clone();

            // Start the client in a background task
            tokio::spawn(async move {
                tokio::select! {
                    result = client.start() => {
                        match result {
                            Ok(()) => {
                                tracing::info!("Discord client stopped");
                            }
                            Err(e) => {
                                tracing::error!("Discord client error: {}", e);
                                *status.write().await = ChannelStatus::Error;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Discord channel shutdown requested");
                        client.shard_manager.shutdown_all().await;
                    }
                }
                *status.write().await = ChannelStatus::Disconnected;
            });

            // Wait a moment for connection
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled (enable 'discord' feature)".to_string(),
            ))
        }
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Discord channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.set_status(ChannelStatus::Disconnected).await;

        #[cfg(feature = "discord")]
        {
            self.http = None;
        }

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        #[cfg(feature = "discord")]
        {
            let http = self
                .http
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

            // Parse channel ID from conversation_id
            // Handle both "dm:user_id" and direct channel IDs
            let channel_id = if message.conversation_id.as_str().starts_with("dm:") {
                // For DMs, we need to create a DM channel first
                let user_id: u64 = message
                    .conversation_id
                    .as_str()
                    .strip_prefix("dm:")
                    .unwrap()
                    .parse()
                    .map_err(|e| ChannelError::SendFailed(format!("Invalid user ID: {}", e)))?;

                let user = serenity::all::UserId::new(user_id);
                let dm_channel = user
                    .create_dm_channel(http)
                    .await
                    .map_err(|e| ChannelError::SendFailed(format!("Failed to create DM channel: {}", e)))?;

                dm_channel.id
            } else {
                SerenityChannelId::new(
                    message
                        .conversation_id
                        .as_str()
                        .parse()
                        .map_err(|e| ChannelError::SendFailed(format!("Invalid channel ID: {}", e)))?,
                )
            };

            // Build message
            let mut builder = CreateMessage::new().content(&message.text);

            // Add reply reference if specified
            if let Some(reply_to) = &message.reply_to {
                if let Ok(msg_id) = reply_to.as_str().parse::<u64>() {
                    builder = builder.reference_message(serenity::all::MessageReference::from((
                        channel_id,
                        serenity::all::MessageId::new(msg_id),
                    )));
                }
            }

            // Add attachments
            for attachment in &message.attachments {
                if let Some(data) = &attachment.data {
                    let filename = attachment
                        .filename
                        .clone()
                        .unwrap_or_else(|| "attachment".to_string());
                    builder = builder.add_file(CreateAttachment::bytes(data.clone(), filename));
                }
            }

            // Send the message
            let sent = channel_id
                .send_message(http, builder)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Discord send error: {}", e)))?;

            Ok(SendResult {
                message_id: MessageId::new(sent.id.to_string()),
                timestamp: Utc
                    .timestamp_opt(sent.timestamp.unix_timestamp(), 0)
                    .single()
                    .unwrap_or_else(Utc::now),
            })
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message;
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled".to_string(),
            ))
        }
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        #[cfg(feature = "discord")]
        {
            let http = self
                .http
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

            // Parse channel ID
            let channel_id_str = if conversation_id.as_str().starts_with("dm:") {
                return Err(ChannelError::UnsupportedFeature(
                    "Typing indicator for DMs requires creating DM channel first".to_string(),
                ));
            } else {
                conversation_id.as_str()
            };

            let channel_id = SerenityChannelId::new(
                channel_id_str
                    .parse()
                    .map_err(|e| ChannelError::Internal(format!("Invalid channel ID: {}", e)))?,
            );

            channel_id
                .broadcast_typing(http)
                .await
                .map_err(|e| ChannelError::Internal(format!("Failed to send typing: {}", e)))?;

            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = conversation_id;
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled".to_string(),
            ))
        }
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        #[cfg(feature = "discord")]
        {
            // Note: Editing requires channel_id which we don't have in this interface
            // Would need to track message_id -> channel_id mapping
            let _ = (message_id, new_text);
            Err(ChannelError::UnsupportedFeature(
                "Message editing requires channel context".to_string(),
            ))
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = (message_id, new_text);
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled".to_string(),
            ))
        }
    }

    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        #[cfg(feature = "discord")]
        {
            // Note: Deleting requires channel_id which we don't have in this interface
            let _ = message_id;
            Err(ChannelError::UnsupportedFeature(
                "Message deletion requires channel context".to_string(),
            ))
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message_id;
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled".to_string(),
            ))
        }
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        #[cfg(feature = "discord")]
        {
            // Note: Reacting requires channel_id which we don't have in this interface
            let _ = (message_id, reaction);
            Err(ChannelError::UnsupportedFeature(
                "Reactions require channel context".to_string(),
            ))
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = (message_id, reaction);
            Err(ChannelError::UnsupportedFeature(
                "Discord support not compiled".to_string(),
            ))
        }
    }
}

/// Factory for creating Discord channels
pub struct DiscordChannelFactory;

#[async_trait]
impl ChannelFactory for DiscordChannelFactory {
    fn channel_type(&self) -> &str {
        "discord"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: DiscordConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Discord config: {}", e)))?;

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        Ok(Box::new(DiscordChannel::new("discord", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = DiscordChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert_eq!(caps.max_message_length, 2000);
    }

    #[test]
    fn test_channel_creation() {
        let config = DiscordConfig {
            bot_token: "test_token_that_is_long_enough_to_pass_validation_check".to_string(),
            ..Default::default()
        };
        let channel = DiscordChannel::new("discord-test", config);
        assert_eq!(channel.info().id.as_str(), "discord-test");
        assert_eq!(channel.info().channel_type, "discord");
    }
}

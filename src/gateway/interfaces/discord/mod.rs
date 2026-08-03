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

pub mod api;
pub mod config;
pub mod permissions;
pub mod resolver;
pub mod security;

pub use config::{DiscordConfig, IntentsConfig};
pub use resolver::{Candidate, ChannelResolutionError, DiscordResolver, ResolvedChannel};

use crate::gateway::channel::{
    Attachment, Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage,
    InboundMessageSender, MessageId, MessageMeta, OutboundMessage, SendResult, UserId,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tokio::sync::{oneshot, RwLock};

use std::collections::HashMap;

use serenity::{
    all::{
        ButtonStyle, ChannelId as SerenityChannelId, CommandDataOptionValue, ComponentInteraction,
        Context, CreateActionRow, CreateAttachment, CreateButton, CreateInteractionResponse,
        CreateMessage, EditMessage, EventHandler, GatewayIntents, GuildChannel, Interaction,
        Message, MessageId as SerenityMessageId, PartialGuildChannel, Ready,
    },
    Client,
};

/// Thread binding - links a Discord thread to a conversation
#[derive(Debug, Clone)]
pub struct ThreadBinding {
    /// Discord thread ID
    pub thread_id: u64,
    /// Bound conversation ID
    pub conversation_id: ConversationId,
    /// Guild ID where the thread exists
    pub guild_id: u64,
    /// Parent channel ID
    pub parent_channel_id: u64,
    /// Thread name
    pub name: String,
    /// When the binding was created
    pub created_at: chrono::DateTime<Utc>,
}

impl ThreadBinding {
    /// Create a new thread binding from thread details
    #[must_use]
    pub fn new(
        thread_id: u64,
        conversation_id: ConversationId,
        guild_id: u64,
        parent_channel_id: u64,
        name: String,
    ) -> Self {
        Self {
            thread_id,
            conversation_id,
            guild_id,
            parent_channel_id,
            name,
            created_at: Utc::now(),
        }
    }
}

/// Discord channel implementation
pub struct DiscordChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: DiscordConfig,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// HTTP client for sending messages (serenity's Http)
    http: Option<Arc<serenity::http::Http>>,
    /// Test mode: skip real gateway connection, return mock results
    test_mode: bool,
}

impl DiscordChannel {
    /// Create a new Discord channel.
    pub fn new(id: impl Into<String>, config: DiscordConfig) -> Self {
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
            channel_state: ChannelState::new(100),
            shutdown_tx: None,
            http: None,
            test_mode: false,
        }
    }

    pub fn for_test(id: impl Into<String>, config: DiscordConfig) -> Self {
        let mut channel = Self::new(id, config);
        channel.test_mode = true;
        channel
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
            // Reply streaming: the generic ReplyEmitter drives send→edit via our
            // `Channel::edit()` impl. The inbound executor flips `stream_enabled`
            // on when it sees EditBased. Mirrors Telegram's progressive editing.
            stream_protocol: crate::gateway::channel::StreamProtocol::EditBased,
        }
    }

    /// Parse a `conversation_id` into a `SerenityChannelId`, handling `dm:user_id` format.
    async fn resolve_channel_id(
        &self,
        conversation_id: &ConversationId,
    ) -> ChannelResult<SerenityChannelId> {
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

        if conversation_id.as_str().starts_with("dm:") {
            let user_id: u64 = conversation_id.as_str()[3..]
                .parse()
                .map_err(|e| ChannelError::Internal(format!("Invalid user ID: {e}")))?;

            let user = serenity::all::UserId::new(user_id);
            let dm = user
                .create_dm_channel(http)
                .await
                .map_err(|e| ChannelError::Internal(format!("Failed to create DM channel: {e}")))?;
            Ok(dm.id)
        } else {
            conversation_id
                .as_str()
                .parse::<u64>()
                .map(SerenityChannelId::new)
                .map_err(|e| ChannelError::Internal(format!("Invalid channel ID: {e}")))
        }
    }

    /// Parse a `MessageId` string into a `SerenityMessageId`.
    fn parse_message_id(message_id: &MessageId) -> ChannelResult<SerenityMessageId> {
        message_id
            .as_str()
            .parse::<u64>()
            .map(SerenityMessageId::new)
            .map_err(|e| ChannelError::Internal(format!("Invalid message ID: {e}")))
    }
}

/// Map an approval `callback_data` to a styled Discord button.
///
/// `approve*` → green (Success), `deny*` → red (Danger), otherwise neutral.
/// The `custom_id` carries `callback_data` verbatim so the click round-trips
/// back to the approval sink unchanged.
fn button_style_for(callback_data: &str, text: &str) -> CreateButton {
    let style = if callback_data.starts_with("approve") {
        ButtonStyle::Success
    } else if callback_data.starts_with("deny") {
        ButtonStyle::Danger
    } else {
        ButtonStyle::Secondary
    };
    CreateButton::new(callback_data).label(text).style(style)
}

/// Event handler for Discord gateway events
struct Handler {
    inbound_tx: InboundMessageSender,
    config: DiscordConfig,
    status: Arc<RwLock<ChannelStatus>>,
    bot_user_id: Arc<RwLock<Option<u64>>>,
    thread_bindings: Arc<RwLock<HashMap<u64, ThreadBinding>>>,
}

impl Handler {
    /// Forward a button/select-menu click to the router's approval sink.
    ///
    /// The router recognises approval callbacks by the `cb_` message-id prefix
    /// and hands `text` (== the button's `custom_id`) plus the clicker id to the
    /// injected `ApprovalCallbackSink`. Discord itself holds no approval state —
    /// this stays R4-pure I/O: translate the interaction into an inbound message.
    async fn handle_component(&self, ctx: &Context, component: &ComponentInteraction) {
        // Enforce the same guild/channel allowlist as messages and commands so a
        // button can't drive the agent from a non-allowed surface.
        if let Some(guild_id) = component.guild_id {
            if !self.config.is_guild_allowed(guild_id.get()) {
                return;
            }
        }
        if component.guild_id.is_none() && !self.config.dm_allowed {
            return;
        }
        if component.guild_id.is_some()
            && !self.config.is_channel_allowed(component.channel_id.get())
        {
            return;
        }

        // Key the conversation exactly like message()/interaction_create so the
        // approval reply lands in the same conversation as the original prompt.
        let conversation_id = if component.guild_id.is_some() {
            ConversationId::new(component.channel_id.to_string())
        } else {
            ConversationId::new(format!("dm:{}", component.user.id))
        };

        let inbound = InboundMessage {
            id: MessageId::new(format!("cb_{}", component.id)),
            channel_id: ChannelId::new("discord"),
            conversation_id,
            sender_id: UserId::new(component.user.id.to_string()),
            sender_name: Some(component.user.name.clone()),
            text: component.data.custom_id.clone(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: component.guild_id.is_some(),
            raw: None,
            metadata: vec![],
        };

        if let Err(e) = self.inbound_tx.send(inbound) {
            tracing::error!(error = ?e, "Failed to forward Discord button callback");
        }

        // ACK the interaction (deferred update) so Discord clears the click
        // spinner. The human-visible result is posted by the router as a normal
        // channel message.
        if let Err(e) = component
            .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
            .await
        {
            tracing::debug!(error = ?e, "Failed to ACK Discord component interaction");
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(
            "Discord bot connected: {}#{} ({})",
            ready.user.name,
            ready
                .user
                .discriminator
                .map(|d| d.to_string())
                .unwrap_or_default(),
            ready.user.id
        );

        // Store bot user ID for mention detection
        *self.bot_user_id.write().await = Some(ready.user.id.get());
        *self.status.write().await = ChannelStatus::Connected;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Self-loop guard: never re-process our own bot's messages.
        // Foreign bots fall through with `MessageMeta::BotAuthored` so the
        // inbound router's pair-loop-guard can suppress sustained storms.
        let bot_self_id = *self.bot_user_id.read().await;
        let is_self = bot_self_id == Some(msg.author.id.get());
        if msg.author.bot && is_self {
            return;
        }
        let is_foreign_bot = msg.author.bot && !is_self;

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
        if !is_dm && !self.config.is_channel_allowed(msg.channel_id.get()) {
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

        // Only process if mentioned or has prefix (for guilds); always process DMs.
        // A prefix always triggers; a mention triggers only when respond_to_mentions
        // is on. `respond_to_mentions = false` must restrict to prefix-only, NOT
        // disable the guard (which would make the bot answer every guild message).
        if !(is_dm || has_prefix || (mentioned && self.config.respond_to_mentions)) {
            return;
        }

        // Extract text (remove mention/prefix if present)
        let text = if has_prefix {
            msg.content[self.config.command_prefix.len()..]
                .trim()
                .to_string()
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
                size: Some(u64::from(a.size)),
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

        let mut metadata: Vec<MessageMeta> = Vec::new();
        if is_foreign_bot {
            metadata.push(MessageMeta::BotAuthored);
        }

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
            metadata,
        };

        // Send to channel
        if let Err(e) = self.inbound_tx.send(inbound) {
            tracing::error!(error = ?e, "Failed to send inbound Discord message");
        }

        // Send typing indicator if enabled
        if self.config.send_typing {
            let _ = msg.channel_id.broadcast_typing(&ctx.http).await;
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        // Button / select-menu clicks (approval UI): forward the callback to the
        // router's approval sink via a cb_-prefixed inbound message, then ACK.
        if let Interaction::Component(component) = &interaction {
            self.handle_component(&ctx, component).await;
            return;
        }

        let serenity::all::Interaction::Command(command) = interaction else {
            return;
        };

        if !self.config.slash_commands_enabled {
            tracing::debug!("Slash command ignored (disabled in config)");
            return;
        }

        if command.guild_id.is_none() && !self.config.dm_allowed {
            tracing::debug!("Slash command from DM ignored (DMs disabled)");
            return;
        }

        if let Some(guild_id) = command.guild_id {
            if !self.config.is_guild_allowed(guild_id.get()) {
                tracing::debug!(
                    "Slash command from guild {} ignored (not in allowlist)",
                    guild_id
                );
                return;
            }
        }

        // Mirror the channel allowlist enforced for regular messages —
        // slash commands must not bypass `allowed_channels`.
        if !self.config.is_channel_allowed(command.channel_id.get()) {
            tracing::debug!(
                "Slash command from channel {} ignored (not in allowlist)",
                command.channel_id
            );
            return;
        }

        tracing::info!(
            "Slash command: /{} from user {}",
            command.data.name,
            command.user.name
        );

        let args: Vec<(String, String)> = command
            .data
            .options
            .iter()
            .filter_map(|opt| {
                let value = match &opt.value {
                    CommandDataOptionValue::String(s) => s.clone(),
                    CommandDataOptionValue::Integer(i) => i.to_string(),
                    CommandDataOptionValue::Boolean(b) => b.to_string(),
                    CommandDataOptionValue::User(u) => u.to_string(),
                    CommandDataOptionValue::Channel(c) => c.to_string(),
                    CommandDataOptionValue::Role(r) => r.to_string(),
                    CommandDataOptionValue::Mentionable(m) => m.to_string(),
                    CommandDataOptionValue::Number(n) => n.to_string(),
                    CommandDataOptionValue::Attachment(a) => a.to_string(),
                    _ => return None,
                };
                Some((opt.name.clone(), value))
            })
            .collect();

        let args_text = args
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");

        let text = format!("/{} {}", command.data.name, args_text);

        // Build conversation ID. Key guild slash-commands by channel (like the
        // message() handler's msg.channel_id) — keying by guild would route a
        // /command to a different conversation than plain text in the same channel
        // and collapse every channel in the guild into one shared conversation.
        let conversation_id = if command.guild_id.is_some() {
            ConversationId::new(command.channel_id.to_string())
        } else {
            ConversationId::new(format!("dm:{}", command.user.id))
        };

        let inbound = InboundMessage {
            id: MessageId::new(command.id.to_string()),
            channel_id: ChannelId::new("discord"),
            conversation_id,
            sender_id: UserId::new(command.user.id.to_string()),
            sender_name: Some(command.user.name.clone()),
            text,
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: command.guild_id.is_some(),
            raw: Some(serde_json::json!({
                "command_id": command.data.id.to_string(),
                "command_name": command.data.name,
                "guild_id": command.guild_id.map(|g| g.to_string()),
                "channel_id": command.channel_id.to_string(),
                "args": args,
            })),
            metadata: vec![],
        };

        if let Err(e) = self.inbound_tx.send(inbound) {
            tracing::error!(error = ?e, "Failed to send inbound Discord interaction");
        }
    }

    async fn thread_create(&self, _ctx: Context, new_channel: GuildChannel) {
        if !self.config.intents.guild_threads {
            return;
        }
        let channel_id = new_channel.id.get();
        tracing::debug!(
            "Thread created: {} in channel {}",
            channel_id,
            new_channel.parent_id.map_or(0, |p| p.get())
        );

        let binding = ThreadBinding::new(
            channel_id,
            ConversationId::new(channel_id.to_string()),
            new_channel.guild_id.get(),
            new_channel.parent_id.map_or(0, |p| p.get()),
            new_channel.name.clone(),
        );

        self.thread_bindings
            .write()
            .await
            .insert(channel_id, binding);
    }

    async fn thread_update(
        &self,
        _ctx: Context,
        _old_channel: Option<GuildChannel>,
        new_channel: GuildChannel,
    ) {
        if !self.config.intents.guild_threads {
            return;
        }
        let channel_id = new_channel.id.get();
        if let Some(binding) = self.thread_bindings.write().await.get_mut(&channel_id) {
            binding.name = new_channel.name.clone();
            tracing::debug!("Thread updated: {} ({})", channel_id, new_channel.name);
        }
    }

    async fn thread_delete(
        &self,
        _ctx: Context,
        channel: PartialGuildChannel,
        _channel_as_thread: Option<GuildChannel>,
    ) {
        if !self.config.intents.guild_threads {
            return;
        }
        let channel_id = channel.id.get();
        if self
            .thread_bindings
            .write()
            .await
            .remove(&channel_id)
            .is_some()
        {
            tracing::debug!("Thread deleted: {}", channel_id);
        }
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config.validate().map_err(ChannelError::ConfigError)?;

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
        tracing::info!("Starting Discord channel...");

        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            tracing::info!("Discord channel started in test mode");
            return Ok(());
        }

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
            inbound_tx: self.channel_state.sender(),
            config: self.config.clone(),
            status: self.channel_state.status_handle(),
            bot_user_id: Arc::new(RwLock::new(None)),
            thread_bindings: Arc::new(RwLock::new(HashMap::new())),
        };

        // Build client
        let mut client = Client::builder(&self.config.bot_token, intents)
            .event_handler(handler)
            .await
            .map_err(|e| {
                ChannelError::ConfigError(format!("Failed to create Discord client: {e}"))
            })?;

        // Store HTTP client for sending messages
        self.http = Some(client.http.clone());

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let status = self.channel_state.status_handle();

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
            // Do NOT clobber `Error`: the unconditional assignment that used to
            // live here erased the failure one line after it was recorded, so a
            // crashed gateway client reported "disconnected" — indistinguishable
            // from a clean shutdown to `channels.list`, and invisible to the
            // health monitor's error-based restart path.
            {
                let mut guard = status.write().await;
                if *guard != ChannelStatus::Error {
                    *guard = ChannelStatus::Disconnected;
                }
            }
        });

        // Wait a moment for connection
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Discord channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;

        self.http = None;

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Discord channel not started".to_string(),
                ));
            }
            return Ok(SendResult {
                message_id: MessageId::new("discord-test-msg-id".to_string()),
                timestamp: chrono::Utc::now(),
            });
        }
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

        // Parse channel ID from conversation_id
        // Handle both "dm:user_id" and direct channel IDs
        let channel_id =
            if message.conversation_id.as_str().starts_with("dm:") {
                // For DMs, we need to create a DM channel first
                let user_id: u64 = message.conversation_id.as_str()[3..]
                    .parse()
                    .map_err(|e| ChannelError::SendFailed(format!("Invalid user ID: {e}")))?;

                let user = serenity::all::UserId::new(user_id);
                let dm_channel = user.create_dm_channel(http).await.map_err(|e| {
                    ChannelError::SendFailed(format!("Failed to create DM channel: {e}"))
                })?;

                dm_channel.id
            } else {
                SerenityChannelId::new(
                    message.conversation_id.as_str().parse().map_err(|e| {
                        ChannelError::SendFailed(format!("Invalid channel ID: {e}"))
                    })?,
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

        // Render inline keyboard (approval UI etc.) as Discord button components.
        // The button `custom_id` carries the callback_data verbatim; a click comes
        // back through `interaction_create` → cb_-prefixed inbound → approval sink.
        // Discord caps action rows at 5 and buttons-per-row at 5.
        if let Some(keyboard) = &message.inline_keyboard {
            let rows: Vec<CreateActionRow> = keyboard
                .rows
                .iter()
                .take(5)
                .filter_map(|row| {
                    let buttons: Vec<CreateButton> = row
                        .iter()
                        .take(5)
                        .map(|btn| button_style_for(&btn.callback_data, &btn.text))
                        .collect();
                    (!buttons.is_empty()).then_some(CreateActionRow::Buttons(buttons))
                })
                .collect();
            if !rows.is_empty() {
                builder = builder.components(rows);
            }
        }

        // Send the message
        let sent = channel_id
            .send_message(http, builder)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Discord send error: {e}")))?;

        Ok(SendResult {
            message_id: MessageId::new(sent.id.to_string()),
            timestamp: Utc
                .timestamp_opt(sent.timestamp.unix_timestamp(), 0)
                .single()
                .unwrap_or_else(Utc::now),
        })
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Discord channel not started".to_string(),
                ));
            }
            return Ok(());
        }
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
                .map_err(|e| ChannelError::Internal(format!("Invalid channel ID: {e}")))?,
        );

        channel_id
            .broadcast_typing(http)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to send typing: {e}")))?;

        Ok(())
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Discord channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        let builder = EditMessage::new().content(new_text);
        channel_id
            .edit_message(http, msg_id, builder)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to edit message: {e}")))?;

        Ok(())
    }

    async fn delete(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Discord channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        channel_id
            .delete_message(http, msg_id)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to delete message: {e}")))?;

        Ok(())
    }

    async fn react(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Discord channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let http = self
            .http
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("HTTP client not initialized".to_string()))?;

        let channel_id = self.resolve_channel_id(conversation_id).await?;
        let msg_id = Self::parse_message_id(message_id)?;

        // Parse emoji — custom format <:name:id> or <a:name:id>, otherwise Unicode
        let reaction_type = if reaction.starts_with('<') {
            serenity::all::ReactionType::try_from(reaction)
                .map_err(|e| ChannelError::Internal(format!("Invalid emoji format: {e}")))?
        } else {
            serenity::all::ReactionType::Unicode(reaction.to_string())
        };

        http.create_reaction(channel_id, msg_id, &reaction_type)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to add reaction: {e}")))?;

        Ok(())
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
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Discord config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

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

    #[test]
    fn test_stream_protocol_is_edit_based() {
        // Reply streaming relies on this: the inbound executor only enables
        // stream_enabled when the channel declares EditBased.
        assert_eq!(
            DiscordChannel::capabilities().stream_protocol,
            crate::gateway::channel::StreamProtocol::EditBased
        );
    }

    #[test]
    fn test_button_style_mapping() {
        // approve* → green, deny* → red, everything else neutral. The label and
        // custom_id (== callback_data) must survive verbatim for the round-trip.
        // We assert via the serialized component JSON since CreateButton exposes
        // no getters.
        let approve =
            serde_json::to_value(button_style_for("approve:abc:once", "Approve")).unwrap();
        assert_eq!(approve["style"], 3); // ButtonStyle::Success
        assert_eq!(approve["custom_id"], "approve:abc:once");
        assert_eq!(approve["label"], "Approve");

        let deny = serde_json::to_value(button_style_for("deny:abc", "Deny")).unwrap();
        assert_eq!(deny["style"], 4); // ButtonStyle::Danger

        let other = serde_json::to_value(button_style_for("misc:x", "Other")).unwrap();
        assert_eq!(other["style"], 2); // ButtonStyle::Secondary
    }
}

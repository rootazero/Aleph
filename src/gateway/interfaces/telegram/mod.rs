//! Telegram Channel Implementation
//!
//! Integrates with the Telegram Bot API using the teloxide framework.
//!
//! # Features
//!
//! - Long-polling or webhook mode
//! - User/group allowlists with pairing flow
//! - File and image attachments with URL resolution
//! - Inline keyboards with callback routing
//! - Reply threading
//! - Forum topic session isolation
//! - Processing status reactions (👀/👍/👎)
//! - Sticker support (static/animated/video)
//! - Network stall detection with watchdog
//! - Smart retry with error classification

pub mod access;
pub mod approval;
pub mod chunking;
pub mod config;
pub mod config_v2;
pub mod config_resolver;
pub mod context;
pub mod delivery;
pub mod error_cooldown;
pub mod group_chat;
pub mod handlers;
pub mod offset;
mod polling;
pub mod bot_instance;
pub mod poll;
pub mod reasoning_lane;
pub mod reaction_handler;
pub mod session;
pub mod status_reaction;
pub mod sticker;

pub use access::AccessController;
pub use bot_instance::BotInstance;
pub use context::{AccessLevel, ChatType, ConversationKey, MediaItem, SessionState, TelegramInboundContext};
pub use session::{SessionConfig, SessionError, TelegramSessionManager};
pub use config_v2::TelegramConfigV2;
pub use config_resolver::{ConfigResolver, ResolvedConfig};
pub use config::{PairingEntry, TelegramConfig, WebhookConfig};
pub use config_v2::{DmPolicy, GroupPolicy, StreamingOptions};

use crate::gateway::channel::{
    CallbackQuery, Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId,
    ChannelInfo, ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage,
    MessageId, MessageMeta, OutboundMessage, PairingData, SendResult, UserId,
};
use access::AccessDecision;
use async_trait::async_trait;
use chrono::Utc;
use error_cooldown::ErrorCooldown;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use teloxide::{prelude::*, types::CallbackQuery as TgCallbackQuery};

/// Telegram channel implementation
pub struct TelegramChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration (multi-account v2)
    config_v2: TelegramConfigV2,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Callback query sender
    callback_tx: mpsc::Sender<CallbackQuery>,
    /// Callback query receiver (taken on first call)
    callback_rx: Option<mpsc::Receiver<CallbackQuery>>,
    /// Active bot instances (one per account)
    bot_instances: Vec<bot_instance::BotInstance>,
    /// ToolRegistry for building slash commands at startup
    tool_registry: Option<Arc<crate::dispatcher::ToolRegistry>>,
    /// Centralized access controller (pairing, allowlists, policies).
    access: Arc<AccessController>,
    /// Per-conversation error cooldown and typing circuit breaker.
    error_cooldown: Arc<ErrorCooldown>,
    /// Persistent polling offset tracker (set via `set_offset_tracker`).
    offset_tracker: Option<Arc<offset::OffsetTracker>>,
    /// State database for pairing persistence (set via `set_state_database`).
    state_db: Option<Arc<crate::resilience::StateDatabase>>,
    /// Multi-account config resolver
    config_resolver: ConfigResolver,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(id: impl Into<String>, config_v2: TelegramConfigV2) -> Self {
        let (callback_tx, callback_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Telegram".to_string(),
            channel_type: "telegram".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        let resolver = ConfigResolver::from_v2(&config_v2);
        let access_config = if let Some(first) = config_v2.accounts.first() {
            resolver.resolve(&first.id, 0, None).cloned().unwrap_or_else(|| ResolvedConfig {
                account_id: first.id.clone(),
                bot_token: first.bot_token.clone(),
                bot_username: first.bot_username.clone(),
                default_agent: first.default_agent.clone(),
                dm_policy: first.dm_policy.clone().unwrap_or_default(),
                group_policy: first.group_policy.clone().unwrap_or_default(),
                send_typing: first.send_typing.unwrap_or(true),
                allowed_users: first.allowed_users.clone().unwrap_or_default(),
                allowed_groups: first.allowed_groups.clone().unwrap_or_default(),
                streaming: first.streaming.clone().unwrap_or_default(),
                error_policy: first.error_policy.clone().unwrap_or_default(),
                max_retries: 3,
            })
        } else {
            ResolvedConfig {
                account_id: "default".to_string(),
                bot_token: String::new(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: true,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 3,
            }
        };
        let access = Arc::new(AccessController::new(access_config));

        Self {
            info,
            config_v2,
            channel_state: ChannelState::new(100),
            callback_tx,
            callback_rx: Some(callback_rx),
            bot_instances: Vec::new(),
            tool_registry: None,
            access,
            error_cooldown: Arc::new(ErrorCooldown::new()),
            offset_tracker: None,
            state_db: None,
            config_resolver: resolver,
        }
    }

    /// Set the ToolRegistry so this channel can query builtin tools at startup
    /// and register them as Telegram slash commands.
    pub fn set_tool_registry(&mut self, registry: Arc<crate::dispatcher::ToolRegistry>) {
        self.tool_registry = Some(registry);
    }

    /// Set the offset tracker for persistent polling offset management.
    pub fn set_offset_tracker(&mut self, tracker: Arc<offset::OffsetTracker>) {
        self.offset_tracker = Some(tracker);
    }

    /// Set the state database for pairing persistence.
    ///
    /// Must be called **before** `start()` so that the `AccessController`
    /// can load and persist paired users.
    pub fn set_state_database(&mut self, db: Arc<crate::resilience::StateDatabase>) {
        self.state_db = Some(db);
    }

    /// Get Telegram-specific capabilities
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
            rich_text: true, // Markdown/HTML support
            max_message_length: 4096,
            max_attachment_size: 50 * 1024 * 1024, // 50MB
            stream_protocol: crate::gateway::channel::StreamProtocol::EditBased,
        }
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

    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        let code = self.access.generate_code().await;
        Ok(PairingData::Code(code))
    }

    async fn list_active_pairing_codes(&self) -> ChannelResult<Vec<(String, u64)>> {
        Ok(self.access.list_codes().await)
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.config_v2.accounts.is_empty() {
            return Err(ChannelError::ConfigError(
                "No Telegram accounts configured".to_string(),
            ));
        }

        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!("Starting Telegram channel...");

        // Inject state database into AccessController for pairing persistence.
        if let Some(ref db) = self.state_db {
            if let Some(access) = Arc::get_mut(&mut self.access) {
                access.set_state_database(db.clone());
            } else {
                tracing::warn!(
                    "Could not inject StateDatabase into AccessController \
                     (Arc has multiple references)"
                );
            }
        }

        // Load previously paired users from the database into memory.
        self.access.load_from_database(self.info.id.as_str()).await;

        for account in &self.config_v2.accounts {
            let resolved_config = self
                .config_resolver
                .resolve(&account.id, 0, None)
                .cloned()
                .unwrap_or_else(|| ResolvedConfig {
                    account_id: account.id.clone(),
                    bot_token: account.bot_token.clone(),
                    bot_username: account.bot_username.clone(),
                    default_agent: account.default_agent.clone(),
                    dm_policy: account.dm_policy.clone().unwrap_or_default(),
                    group_policy: account.group_policy.clone().unwrap_or_default(),
                    send_typing: account.send_typing.unwrap_or(true),
                    allowed_users: account.allowed_users.clone().unwrap_or_default(),
                    allowed_groups: account.allowed_groups.clone().unwrap_or_default(),
                    streaming: account.streaming.clone().unwrap_or_default(),
                    error_policy: account.error_policy.clone().unwrap_or_default(),
                    max_retries: 3,
                });

            let mut instance =
                BotInstance::new(account, self.callback_tx.clone(), resolved_config);

            // Verify bot token by getting bot info
            match instance.bot.get_me().await {
                Ok(me) => {
                    tracing::info!(
                        account_id = %account.id,
                        username = %me.username(),
                        id = %me.id,
                        "Telegram bot connected"
                    );

                    // Boot diagnostics — fire-and-forget
                    {
                        let diag_bot = instance.bot.clone();
                        let can_read_groups = me.can_read_all_group_messages;
                        let group_ids: Vec<i64> = instance.resolved_config.allowed_groups.clone();

                        tokio::spawn(async move {
                            let mut warnings: Vec<String> = Vec::new();

                            // (a) Privacy mode check
                            if !can_read_groups {
                                warnings.push(
                                    "Privacy mode is enabled. Talk to @BotFather and disable \
                                     privacy mode for group message access"
                                        .to_string(),
                                );
                            }

                            // (b) Group reachability check
                            let total_groups = group_ids.len();
                            let mut reachable = 0usize;
                            for gid in &group_ids {
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    diag_bot.get_chat(teloxide::types::ChatId(*gid)),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => {
                                        reachable += 1;
                                    }
                                    Ok(Err(e)) => {
                                        warnings.push(format!("Group {} unreachable: {}", gid, e));
                                    }
                                    Err(_) => {
                                        warnings.push(format!("Group {} check timed out", gid));
                                    }
                                }
                            }

                            // Log summary
                            if warnings.is_empty() {
                                tracing::info!(
                                    privacy_mode_disabled = can_read_groups,
                                    groups_reachable = %format!("{}/{}", reachable, total_groups),
                                    "Telegram boot diagnostics: all checks passed"
                                );
                            } else {
                                tracing::warn!(
                                    privacy_mode_disabled = can_read_groups,
                                    groups_reachable = %format!("{}/{}", reachable, total_groups),
                                    warnings = ?warnings,
                                    "Telegram boot diagnostics: issues detected"
                                );
                            }
                        });
                    }
                }
                Err(e) => {
                    self.set_status(ChannelStatus::Error).await;
                    return Err(ChannelError::AuthFailed(format!(
                        "Failed to verify bot token for account {}: {}",
                        account.id, e
                    )));
                }
            }

            // Build slash commands from ToolRegistry (user-facing commands only)
            if let Some(ref registry) = self.tool_registry {
                use teloxide::types::BotCommand;

                let tools = registry.list_builtin_tools().await;
                let mut commands: Vec<(String, String)> = tools
                    .iter()
                    .filter(|t| t.usage.is_some())
                    .map(|t| (t.name.clone(), t.description.clone()))
                    .collect();

                let aliases = [
                    ("image", "Generate an image from a text prompt"),
                    ("video", "Generate a video from a text prompt"),
                    ("audio", "Generate audio/music from a text prompt"),
                    ("speech", "Convert text to speech"),
                ];
                for (alias, desc) in aliases {
                    if !commands.iter().any(|(name, _)| name == alias) {
                        commands.push((alias.to_string(), desc.to_string()));
                    }
                }

                let bot_commands: Vec<BotCommand> = commands
                    .iter()
                    .take(100)
                    .filter_map(|(name, desc)| {
                        let normalized: String = name
                            .to_lowercase()
                            .chars()
                            .map(|c| if c == '-' { '_' } else { c })
                            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
                            .take(32)
                            .collect();
                        if normalized.is_empty() {
                            return None;
                        }
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
                    let _ = instance.bot.delete_my_commands().await;
                    let cmd_names: Vec<_> =
                        bot_commands.iter().map(|c| c.command.as_str()).collect();
                    tracing::debug!("Telegram slash commands to register: {:?}", cmd_names);
                    match instance.bot.set_my_commands(bot_commands.clone()).await {
                        Ok(_) => {
                            tracing::info!(
                                "Registered {} slash commands with Telegram Bot API: {:?}",
                                bot_commands.len(),
                                cmd_names,
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to register Telegram slash commands: {} (bot will still work)",
                                e
                            );
                        }
                    }
                }
            }

            if let Some(ref tracker) = self.offset_tracker {
                instance.set_offset_tracker(tracker.clone());
            }

            // Build handler closures capturing channel-specific Arc clones
            let inbound_tx = self.channel_state.sender();
            let inbound_tx_for_cb = self.channel_state.sender();
            let callback_tx = self.callback_tx.clone();
            let channel_id = self.info.id.clone();
            let channel_id_for_cb = self.info.id.clone();

            let access_clone = self.access.clone();
            let access_for_cb = self.access.clone();

            let state_db_for_sticker = self.state_db.clone();

            let message_handler =
                Update::filter_message().endpoint(move |bot: Bot, msg: teloxide::types::Message| {
                    let inbound_tx = inbound_tx.clone();
                    let channel_id = channel_id.clone();
                    let access = access_clone.clone();
                    let sticker_pipeline = sticker::StickerPipeline::new(state_db_for_sticker.clone());
                    async move {
                        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
                        let is_group = msg.chat.is_group() || msg.chat.is_supergroup();
                        let chat_id = msg.chat.id.0;

                        match access.check_message(user_id, chat_id, is_group).await {
                            AccessDecision::Allowed => {
                                if let Some(inbound) =
                                    handlers::convert_message(&msg, &bot, &channel_id, &sticker_pipeline).await
                                {
                                    let tg_ctx = TelegramInboundContext::from_inbound(
                                        inbound.clone(),
                                        &msg,
                                    );
                                    let access_level = AccessLevel::Member;
                                    let _tg_ctx = tg_ctx.with_access_level(access_level);

                                    tracing::debug!(
                                        channel = "telegram",
                                        chat_id = %chat_id,
                                        thread_id = ?_tg_ctx.thread_id,
                                        access_level = ?access_level,
                                        "TelegramInboundContext ready for message"
                                    );

                                    if let Err(e) = inbound_tx.send(inbound) {
                                        tracing::error!("Failed to send inbound message: {:?}", e);
                                    }
                                }
                            }
                            AccessDecision::NeedsPairing => {
                                if let Some(inbound) =
                                    handlers::convert_message(&msg, &bot, &channel_id, &sticker_pipeline).await
                                {
                                    let tg_ctx = TelegramInboundContext::from_inbound(
                                        inbound.clone(),
                                        &msg,
                                    );
                                    let access_level = AccessLevel::Stranger;
                                    let _tg_ctx = tg_ctx.with_access_level(access_level);

                                    tracing::debug!(
                                        channel = "telegram",
                                        chat_id = %chat_id,
                                        thread_id = ?_tg_ctx.thread_id,
                                        access_level = ?access_level,
                                        "TelegramInboundContext ready for needs-pairing message"
                                    );

                                    if let Err(e) = inbound_tx.send(inbound) {
                                        tracing::error!("Failed to send inbound message: {:?}", e);
                                    }
                                }
                            }
                            AccessDecision::Denied => {
                                tracing::debug!(
                                    "Access denied for user {} in chat {}",
                                    user_id,
                                    chat_id
                                );
                            }
                        }
                        Ok::<(), std::convert::Infallible>(())
                    }
                });

            let callback_handler =
                Update::filter_callback_query().endpoint(move |bot: Bot, q: TgCallbackQuery| {
                    let tx = callback_tx.clone();
                    let inbound_tx = inbound_tx_for_cb.clone();
                    let channel_id = channel_id_for_cb.clone();
                    let access = access_for_cb.clone();
                    async move {
                        let (raw_chat_id, thread_id_val) = q
                            .message
                            .as_ref()
                            .map(|m| {
                                let chat = m.chat().id.0;
                                let tid = match m {
                                    teloxide::types::MaybeInaccessibleMessage::Regular(msg) => {
                                        msg.thread_id.map(|t| t.0 .0)
                                    }
                                    _ => None,
                                };
                                (chat, tid)
                            })
                            .unwrap_or((0, None));

                        let conv_id_str = if let Some(tid) = thread_id_val {
                            format!("{}:topic:{}", raw_chat_id, tid)
                        } else {
                            raw_chat_id.to_string()
                        };

                        let msg_id_str = q
                            .message
                            .as_ref()
                            .map(|m| m.id().to_string())
                            .unwrap_or_default();

                        if let Some(data) = q.data.clone() {
                            let user_id_val = q.from.id.0 as i64;

                            let query = CallbackQuery {
                                id: q.id.clone(),
                                user_id: UserId::new(q.from.id.to_string()),
                                chat_id: ConversationId::new(conv_id_str.clone()),
                                message_id: MessageId::new(msg_id_str),
                                data: data.clone(),
                            };
                            if let Err(e) = tx.send(query).await {
                                tracing::error!("Failed to send callback query: {}", e);
                            }

                            let is_group = raw_chat_id < 0;
                            let decision = access
                                .check_message(user_id_val, raw_chat_id, is_group)
                                .await;
                            if decision == AccessDecision::Allowed {
                                let inbound = InboundMessage {
                                    id: MessageId::new(format!("cb_{}", q.id)),
                                    channel_id: channel_id.clone(),
                                    conversation_id: ConversationId::new(conv_id_str),
                                    sender_id: UserId::new(q.from.id.to_string()),
                                    sender_name: q
                                        .from
                                        .username
                                        .clone()
                                        .or_else(|| Some(q.from.first_name.clone())),
                                    text: data,
                                    attachments: Vec::new(),
                                    timestamp: Utc::now(),
                                    reply_to: None,
                                    is_group,
                                    raw: None,
                                    metadata: vec![],
                                };
                                if let Err(e) = inbound_tx.send(inbound) {
                                    tracing::error!(
                                        "Failed to re-inject callback as inbound message: {:?}",
                                        e
                                    );
                                }
                            }
                        }

                        if let Err(e) = bot.answer_callback_query(&q.id).await {
                            tracing::warn!("Failed to answer callback query: {}", e);
                        }

                        Ok::<(), std::convert::Infallible>(())
                    }
                });

            let inbound_tx_for_poll = self.channel_state.sender();
            let channel_id_for_poll = self.info.id.clone();
            let poll_handler = Update::filter_poll_answer().endpoint(
                move |q: teloxide::types::PollAnswer| {
                    let inbound_tx = inbound_tx_for_poll.clone();
                    let channel_id = channel_id_for_poll.clone();
                    async move {
                        let (conversation_id, sender_id, sender_name) = match &q.voter {
                            teloxide::types::Voter::User(user) => (
                                ConversationId::new(user.id.to_string()),
                                UserId::new(user.id.to_string()),
                                user.username.clone().or_else(|| Some(user.first_name.clone())),
                            ),
                            teloxide::types::Voter::Chat(chat) => (
                                ConversationId::new(chat.id.0.to_string()),
                                UserId::new(chat.id.0.to_string()),
                                chat.title().map(|t| t.to_string()),
                            ),
                        };

                        let inbound = InboundMessage {
                            id: MessageId::new(format!("poll_{}", q.poll_id)),
                            channel_id,
                            conversation_id,
                            sender_id,
                            sender_name,
                            text: format!("Poll answer: {:?}", q.option_ids),
                            attachments: Vec::new(),
                            timestamp: chrono::Utc::now(),
                            reply_to: None,
                            is_group: false,
                            raw: None,
                            metadata: vec![MessageMeta::PollAnswer {
                                poll_id: q.poll_id,
                                option_ids: q.option_ids,
                            }],
                        };
                        let _ = inbound_tx.send(inbound);
                        Ok::<(), std::convert::Infallible>(())
                    }
                },
            );

            let inbound_tx_for_reaction = self.channel_state.sender();
            let channel_id_for_reaction = self.info.id.clone();
            let reaction_handler = Update::filter_message_reaction_updated().endpoint(
                move |update: teloxide::types::MessageReactionUpdated| {
                    let inbound_tx = inbound_tx_for_reaction.clone();
                    let channel_id = channel_id_for_reaction.clone();
                    async move {
                        if let Some(inbound) =
                            reaction_handler::convert_reaction(&update, channel_id.as_str())
                        {
                            let _ = inbound_tx.send(inbound);
                        }
                        Ok::<(), std::convert::Infallible>(())
                    }
                },
            );

            let handler = dptree::entry()
                .branch(message_handler)
                .branch(callback_handler)
                .branch(poll_handler)
                .branch(reaction_handler);

            let status = self.channel_state.status_handle();
            let offset = instance.offset_tracker.clone();
            let ec = self.error_cooldown.clone();
            let bot = instance.bot.clone();

            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            tokio::spawn(polling::run_polling_loop(
                bot,
                handler,
                status,
                shutdown_rx,
                offset,
                ec,
            ));

            instance.shutdown_tx = Some(shutdown_tx);
            self.bot_instances.push(instance);
        }

        self.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Telegram channel...");

        for instance in &mut self.bot_instances {
            if let Some(shutdown_tx) = instance.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
        }

        self.set_status(ChannelStatus::Disconnected).await;

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(message.conversation_id.as_str());
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .or(self.bot_instances.first())
            .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;

        delivery::send_message(
            &instance.bot,
            &instance.resolved_config,
            &message,
            &self.error_cooldown,
        )
        .await
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str());
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .or(self.bot_instances.first())
            .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;
        delivery::send_typing(
            &instance.bot,
            conversation_id.as_str(),
            &instance.resolved_config,
            &self.error_cooldown,
        )
        .await
    }

    async fn react(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str());
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .or(self.bot_instances.first())
            .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;
        delivery::send_reaction(
            &instance.bot,
            conversation_id.as_str(),
            message_id,
            reaction,
        )
        .await
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str());
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .or(self.bot_instances.first())
            .ok_or_else(|| ChannelError::NotConnected("No bot instances".to_string()))?;
        delivery::edit_message(
            &instance.bot,
            conversation_id.as_str(),
            message_id,
            Some(new_text),
            None,
        )
        .await
    }

    async fn delete(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        // Note: Deleting requires both message_id and chat_id
        let _ = message_id;
        Err(ChannelError::UnsupportedFeature(
            "Message deletion requires chat context".to_string(),
        ))
    }

    fn approval_capability(
        &self,
    ) -> Option<Arc<dyn crate::gateway::channel_approval::ChannelApprovalCapability>> {
        let first = self.bot_instances.first()?;
        let instance = bot_instance::BotInstance {
            account_id: first.account_id.clone(),
            bot: first.bot.clone(),
            resolved_config: first.resolved_config.clone(),
            callback_tx: first.callback_tx.clone(),
            channel_state: ChannelState::new(100),
            offset_tracker: first.offset_tracker.clone(),
            shutdown_tx: None,
        };
        Some(Arc::new(
            crate::gateway::interfaces::telegram::approval::TelegramChannelApprovalCapability::new(
                Arc::new(TelegramChannel {
                    info: self.info.clone(),
                    config_v2: self.config_v2.clone(),
                    channel_state: ChannelState::new(100),
                    callback_tx: self.callback_tx.clone(),
                    callback_rx: None,
                    bot_instances: vec![instance],
                    tool_registry: self.tool_registry.clone(),
                    access: self.access.clone(),
                    error_cooldown: self.error_cooldown.clone(),
                    offset_tracker: self.offset_tracker.clone(),
                    state_db: self.state_db.clone(),
                    config_resolver: self.config_resolver.clone(),
                }),
                self.access.clone(),
            ),
        ))
    }
}

impl TelegramChannel {
    /// Take the callback receiver (can only be called once)
    pub fn take_callback_receiver(&mut self) -> Option<mpsc::Receiver<CallbackQuery>> {
        self.callback_rx.take()
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
        let config: TelegramConfigV2 = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Telegram config: {}", e)))?;

        Ok(Box::new(TelegramChannel::new("telegram", config)))
    }
}

fn telegram_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> crate::gateway::channel::ChannelResult<std::sync::Arc<dyn ChannelFactory>> {
    Ok(std::sync::Arc::new(TelegramChannelFactory))
}

pub fn register_with_plugin() {
    crate::gateway::interfaces::plugin::register("telegram", telegram_factory_creator);
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
        let config = TelegramConfigV2 {
            accounts: vec![crate::gateway::interfaces::telegram::config_v2::TelegramAccountConfig {
                id: "default".to_string(),
                bot_token: "123:ABC".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let channel = TelegramChannel::new("telegram-test", config);
        assert_eq!(channel.info().id.as_str(), "telegram-test");
        assert_eq!(channel.info().channel_type, "telegram");
    }
}

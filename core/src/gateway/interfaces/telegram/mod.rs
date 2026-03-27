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

pub mod config;
pub mod group_chat;
pub mod handlers;
pub mod message_ops;

pub use config::{PairingEntry, TelegramConfig, WebhookConfig};
pub use message_ops::TelegramMessageOps;

use crate::gateway::channel::{
    Attachment, CallbackQuery, Channel, ChannelCapabilities, ChannelError, ChannelFactory,
    ChannelId, ChannelInfo, ChannelResult, ChannelState, ChannelStatus, ConversationId,
    InboundMessage, InlineKeyboard, MessageId, OutboundMessage, PairingData, SendResult, UserId,
};
use crate::gateway::formatter::{MessageFormatter, MarkupFormat};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;

use teloxide::{
    prelude::*,
    types::{
        CallbackQuery as TgCallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup,
        InputFile, ParseMode, ThreadId,
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
    /// ToolRegistry for building slash commands at startup
    tool_registry: Option<Arc<crate::dispatcher::ToolRegistry>>,
    /// Active pairing codes (code → entry). Shared with handler closure.
    pairing_codes: Arc<RwLock<HashMap<String, PairingEntry>>>,
    /// Rate-limit map: user_id → last prompt time. Avoids spamming unauthorized users.
    pairing_prompt_times: Arc<RwLock<HashMap<i64, Instant>>>,
    /// Users authorized at runtime via pairing (in-memory only).
    runtime_allowed_users: Arc<RwLock<Vec<i64>>>,
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
            tool_registry: None,
            pairing_codes: Arc::new(RwLock::new(HashMap::new())),
            pairing_prompt_times: Arc::new(RwLock::new(HashMap::new())),
            runtime_allowed_users: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Set the ToolRegistry so this channel can query builtin tools at startup
    /// and register them as Telegram slash commands.
    pub fn set_tool_registry(&mut self, registry: Arc<crate::dispatcher::ToolRegistry>) {
        self.tool_registry = Some(registry);
    }

    /// Parse a conversation_id that may contain a forum topic suffix.
    ///
    /// Format: `"{chat_id}"` or `"{chat_id}:topic:{thread_id}"`.
    /// Returns the `ChatId` and an optional raw thread id (i32).
    fn parse_conversation_id(conv_id: &str) -> (ChatId, Option<i32>) {
        if let Some((chat, topic)) = conv_id.split_once(":topic:") {
            (
                ChatId(chat.parse().unwrap_or(0)),
                topic.parse().ok(),
            )
        } else {
            (ChatId(conv_id.parse().unwrap_or(0)), None)
        }
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
        }
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        self.channel_state.set_status(status).await;
    }
}

/// Classification of Telegram API errors for retry logic.
#[derive(Debug)]
enum ErrorClass {
    /// Transient error (network, server-side) — safe to retry.
    Recoverable,
    /// Permanent error (bad request, unauthorized) — do not retry.
    Unrecoverable,
    /// Rate limited by Telegram — wait the given seconds before retrying.
    RateLimited(u64),
}

/// Classify a teloxide request error for retry decisions.
fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        teloxide::RequestError::Api(api_err) => {
            let msg = api_err.to_string();
            if msg.contains("Too Many Requests") || msg.contains("429") {
                ErrorClass::RateLimited(30)
            } else if msg.contains("Unauthorized") || msg.contains("401")
                || msg.contains("Bad Request") || msg.contains("400") {
                ErrorClass::Unrecoverable
            } else {
                ErrorClass::Recoverable
            }
        }
        teloxide::RequestError::Network(_) => ErrorClass::Recoverable,
        _ => ErrorClass::Recoverable,
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
        use rand::Rng;

        // Generate a 6-character alphanumeric code
        let code: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(6)
            .map(char::from)
            .collect::<String>()
            .to_uppercase();

        let entry = PairingEntry::new(code.clone());
        let mut codes = self.pairing_codes.write().await;
        codes.insert(code.clone(), entry);
        // Clean up expired entries
        codes.retain(|_, e| !e.is_expired());
        drop(codes);

        Ok(PairingData::Code(code))
    }

    async fn list_active_pairing_codes(&self) -> ChannelResult<Vec<(String, u64)>> {
        let mut codes = self.pairing_codes.write().await;
        // Clean up expired entries first
        codes.retain(|_, e| !e.is_expired());
        let result = codes
            .values()
            .map(|e| {
                let elapsed = e.created_at.elapsed().as_secs();
                let remaining = e.ttl_secs.saturating_sub(elapsed);
                (e.code.clone(), remaining)
            })
            .collect();
        Ok(result)
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

        // Build slash commands from ToolRegistry (builtin tools only)
        if let Some(ref registry) = self.tool_registry {
            use teloxide::types::BotCommand;

            let tools = registry.list_builtin_tools().await;
            let mut commands: Vec<(String, String)> = tools.iter()
                .map(|t| (t.name.clone(), t.description.clone()))
                .collect();

            // Add shorthand aliases for generation tools
            // These match slash_command.rs shorthand mappings
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

            // Telegram limits: max 100 commands, command name max 32 chars,
            // lowercase a-z, 0-9, underscore only
            let bot_commands: Vec<BotCommand> = commands.iter()
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
                // Clear old commands first, then set new ones
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
        let inbound_tx_for_cb = self.channel_state.sender();
        let callback_tx = self.callback_tx.clone();
        let config = self.config.clone();
        let config_for_cb = self.config.clone();
        let status = self.channel_state.status_handle();
        let channel_id = self.info.id.clone();
        let channel_id_for_cb = self.info.id.clone();

        // Clone pairing state for the message handler closure
        let pairing_codes_clone = self.pairing_codes.clone();
        let prompt_times_clone = self.pairing_prompt_times.clone();
        let runtime_users_clone = self.runtime_allowed_users.clone();
        let runtime_users_for_cb = self.runtime_allowed_users.clone();

        tokio::spawn(async move {
            tracing::info!("Starting Telegram long-polling...");
            *status.write().await = ChannelStatus::Connected;

            let mut attempt = 0u32;
            let mut healthy_since: Option<Instant> = None;

            loop {
                attempt += 1;

                // Re-clone captured variables for this iteration's handler closures
                let iter_inbound_tx = inbound_tx.clone();
                let iter_inbound_tx_for_cb = inbound_tx_for_cb.clone();
                let iter_callback_tx = callback_tx.clone();
                let iter_config = config.clone();
                let iter_config_for_cb = config_for_cb.clone();
                let iter_channel_id = channel_id.clone();
                let iter_channel_id_for_cb = channel_id_for_cb.clone();
                let iter_pairing_codes = pairing_codes_clone.clone();
                let iter_prompt_times = prompt_times_clone.clone();
                let iter_runtime_users = runtime_users_clone.clone();
                let iter_runtime_users_for_cb = runtime_users_for_cb.clone();

                // Message handler
                let message_handler = Update::filter_message().endpoint(
                    move |bot: Bot, msg: teloxide::types::Message| {
                        let inbound_tx = iter_inbound_tx.clone();
                        let config = iter_config.clone();
                        let channel_id = iter_channel_id.clone();
                        let pairing_codes = iter_pairing_codes.clone();
                        let prompt_times = iter_prompt_times.clone();
                        let runtime_users = iter_runtime_users.clone();
                        async move {
                            if let Some(inbound) = handlers::convert_message(
                                &msg, &bot, &config, &channel_id, &runtime_users,
                            ).await {
                                if let Err(e) = inbound_tx.send(inbound).await {
                                    tracing::error!("Failed to send inbound message: {}", e);
                                }
                            } else if let Some(from) = &msg.from {
                                // Message was rejected (unauthorized user or service message).
                                // Handle pairing flow for DM messages from unauthorized users.
                                let user_id = from.id.0 as i64;
                                let is_dm = !msg.chat.is_group() && !msg.chat.is_supergroup();
                                let has_allowlist = !config.allowed_users.is_empty()
                                    || !runtime_users.read().await.is_empty();

                                if is_dm && has_allowlist && !config.is_user_allowed(user_id) {
                                    if let Some(text) = msg.text() {
                                        let code = text.trim().to_uppercase();
                                        let mut codes = pairing_codes.write().await;
                                        if let Some(entry) = codes.get(&code) {
                                            if !entry.is_expired() {
                                                codes.remove(&code);
                                                drop(codes);
                                                runtime_users.write().await.push(user_id);
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    "Paired successfully! You can now send messages.",
                                                ).await;
                                                tracing::info!(
                                                    "User {} paired via code {}",
                                                    user_id,
                                                    code
                                                );
                                            } else {
                                                codes.remove(&code);
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    "Pairing code expired. Please request a new one.",
                                                ).await;
                                            }
                                        } else {
                                            // Rate-limited prompt: once per 5 minutes per user
                                            let mut times = prompt_times.write().await;
                                            let should_prompt = times
                                                .get(&user_id)
                                                .map(|t| t.elapsed().as_secs() > 300)
                                                .unwrap_or(true);
                                            if should_prompt {
                                                times.insert(user_id, Instant::now());
                                                let _ = bot.send_message(
                                                    msg.chat.id,
                                                    "Please enter your pairing code.",
                                                ).await;
                                            }
                                        }
                                    }
                                }
                            }
                            Ok::<(), std::convert::Infallible>(())
                        }
                    },
                );

                // Callback query handler — also re-injects callback data as an
                // InboundMessage so the inbound router can process namespace
                // sub-command selections through the normal message pipeline.
                let callback_handler = Update::filter_callback_query().endpoint(
                    move |bot: Bot, q: TgCallbackQuery| {
                        let tx = iter_callback_tx.clone();
                        let inbound_tx = iter_inbound_tx_for_cb.clone();
                        let config = iter_config_for_cb.clone();
                        let channel_id = iter_channel_id_for_cb.clone();
                        let runtime_users = iter_runtime_users_for_cb.clone();
                        async move {
                            // Extract chat_id and optional thread_id for forum topic isolation
                            let (raw_chat_id, thread_id_val) = q.message.as_ref()
                                .map(|m| {
                                    let chat = m.chat().id.0;
                                    // Extract thread_id from Regular messages for forum topics
                                    let tid = match m {
                                        teloxide::types::MaybeInaccessibleMessage::Regular(msg) => {
                                            msg.thread_id.map(|t| t.0.0)
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

                                // Send to callback channel (for existing consumers)
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

                                // Re-inject as InboundMessage if user is allowed
                                // (static config or runtime-paired)
                                let rt_allowed = runtime_users.read().await.contains(&user_id_val);
                                if config.is_user_allowed(user_id_val) || rt_allowed {
                                    let inbound = InboundMessage {
                                        id: MessageId::new(format!("cb_{}", q.id)),
                                        channel_id: channel_id.clone(),
                                        conversation_id: ConversationId::new(conv_id_str),
                                        sender_id: UserId::new(q.from.id.to_string()),
                                        sender_name: q.from.username.clone().or_else(|| Some(q.from.first_name.clone())),
                                        text: data,
                                        attachments: Vec::new(),
                                        timestamp: Utc::now(),
                                        reply_to: None,
                                        is_group: false,
                                        raw: None,
                                    };
                                    if let Err(e) = inbound_tx.send(inbound).await {
                                        tracing::error!("Failed to re-inject callback as inbound message: {}", e);
                                    }
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

                let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
                    .build();

                // Watchdog: periodic health check via get_me() API call.
                // Previous approach tracked "last message received" which falsely
                // triggered restarts during idle periods (no users messaging).
                // Now we actively probe the API — only restart on real failures.
                const HEALTH_CHECK_INTERVAL_SECS: u64 = 120;
                const MAX_CONSECUTIVE_FAILURES: u32 = 3;

                let (stall_tx, mut stall_rx) = tokio::sync::mpsc::channel::<()>(1);
                let watchdog_cancel = CancellationToken::new();
                let watchdog_token = watchdog_cancel.clone();
                let watchdog_bot = bot.clone();
                let _watchdog = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(
                        std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS),
                    );
                    let mut consecutive_failures: u32 = 0;
                    loop {
                        tokio::select! {
                            _ = interval.tick() => {
                                match watchdog_bot.get_me().await {
                                    Ok(_) => {
                                        if consecutive_failures > 0 {
                                            tracing::info!(
                                                "Telegram health check recovered after {} failures",
                                                consecutive_failures,
                                            );
                                        }
                                        consecutive_failures = 0;
                                    }
                                    Err(e) => {
                                        consecutive_failures += 1;
                                        tracing::warn!(
                                            failures = consecutive_failures,
                                            "Telegram health check failed: {}",
                                            e,
                                        );
                                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                            tracing::error!(
                                                failures = consecutive_failures,
                                                "Telegram health check failed {} consecutive times — triggering restart",
                                                consecutive_failures,
                                            );
                                            let _ = stall_tx.send(()).await;
                                            break;
                                        }
                                    }
                                }
                            }
                            _ = watchdog_token.cancelled() => break,
                        }
                    }
                });

                let which = tokio::select! {
                    _ = dispatcher.dispatch() => "stopped",
                    _ = &mut shutdown_rx => "shutdown",
                    _ = stall_rx.recv() => "stall",
                };
                watchdog_cancel.cancel();

                if which == "shutdown" {
                    tracing::info!("Telegram channel shutdown requested");
                    break;
                }

                // Dispatcher stopped unexpectedly or health check failed — auto-restart
                *status.write().await = ChannelStatus::Connecting;
                tracing::error!(attempt = attempt, reason = which, "Telegram polling {} — auto-restarting", which);

                // Reset attempt counter if we were healthy for >5 minutes
                if healthy_since.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(300)) {
                    attempt = 1;
                }
                let delay = std::cmp::min(5 * 2u64.pow(attempt.saturating_sub(1).min(4)), 60);
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

                healthy_since = Some(Instant::now());

                tracing::info!(attempt = attempt, "Telegram reconnected, queued messages will be delivered");
                *status.write().await = ChannelStatus::Connected;
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

        let (chat_id, thread_id) = Self::parse_conversation_id(message.conversation_id.as_str());

        // Send typing indicator if enabled
        if self.config.send_typing {
            let mut action_req = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
            if let Some(tid) = thread_id {
                if tid != 1 {
                    action_req = action_req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
                }
            }
            let _ = action_req.await;
        }

        // Voice-only: if text is empty but attachments exist, skip text and send attachments only
        if message.text.is_empty() && !message.attachments.is_empty() {
            let mut first_msg_id = None;
            for attachment in &message.attachments {
                let result = self.send_attachment(bot, chat_id, thread_id, attachment).await;
                if let Err(e) = result {
                    tracing::warn!("Failed to send voice attachment: {}", e);
                }
                if first_msg_id.is_none() {
                    // Use a placeholder message ID for the first attachment
                    first_msg_id = Some("0".to_string());
                }
            }
            return Ok(SendResult {
                message_id: MessageId::new(first_msg_id.unwrap_or_else(|| "0".to_string())),
                timestamp: Utc::now(),
            });
        }

        // Split long messages to respect Telegram's 4096-char limit.
        // Use a conservative limit (3500) to leave room for HTML tag expansion.
        const SPLIT_LIMIT: usize = 3500;
        let chunks = MessageFormatter::split(&message.text, SPLIT_LIMIT);

        // Helper to build a SendMessage request with optional thread routing
        let build_request =
            |parse_mode: Option<ParseMode>,
             text: &str,
             reply_to: Option<&str>,
             keyboard: Option<&InlineKeyboard>| {
                let mut req = bot.send_message(chat_id, text);
                if let Some(mode) = parse_mode {
                    req = req.parse_mode(mode);
                }
                if let Some(reply_to) = reply_to {
                    if let Ok(msg_id) = reply_to.parse::<i32>() {
                        req = req.reply_parameters(teloxide::types::ReplyParameters::new(
                            teloxide::types::MessageId(msg_id),
                        ));
                    }
                }
                // Forum topic: route reply into the correct thread
                if let Some(tid) = thread_id {
                    if tid != 1 {
                        // General topic — do NOT set message_thread_id
                        req =
                            req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
                    }
                }
                if let Some(keyboard) = keyboard {
                    let markup = InlineKeyboardMarkup::new(
                        keyboard.rows.iter().map(|row| {
                            row.iter()
                                .map(|btn| {
                                    InlineKeyboardButton::callback(
                                        &btn.text,
                                        &btn.callback_data,
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    );
                    req = req.reply_markup(markup);
                }
                req
            };

        // Send each chunk with retry logic. Only the first chunk carries
        // reply_to and inline_keyboard; subsequent chunks are plain continuations.
        let max_retries = self.config.max_retries;
        let mut first_msg: Option<teloxide::types::Message> = None;

        for (i, chunk) in chunks.iter().enumerate() {
            let is_first = i == 0;
            let is_last = i == chunks.len() - 1;
            let html_text =
                MessageFormatter::format(chunk, MarkupFormat::TelegramHtml);
            let reply_to_ref = if is_first {
                message.reply_to.as_ref().map(|id| id.as_str())
            } else {
                None
            };
            let keyboard_ref = if is_last {
                message.inline_keyboard.as_ref()
            } else {
                None
            };

            let mut attempts = 0u32;
            let sent = loop {
                let result = build_request(
                    Some(ParseMode::Html),
                    &html_text,
                    reply_to_ref,
                    keyboard_ref,
                )
                .await;
                match result {
                    Ok(msg) => break msg,
                    Err(e) => {
                        attempts += 1;
                        match classify_error(&e) {
                            ErrorClass::Unrecoverable => {
                                // Try plain text fallback
                                tracing::warn!(
                                    "HTML send failed (unrecoverable), retrying as plain text: {}",
                                    e
                                );
                                break build_request(
                                    None,
                                    chunk,
                                    reply_to_ref,
                                    keyboard_ref,
                                )
                                .await
                                .map_err(|e| {
                                    ChannelError::SendFailed(format!(
                                        "Telegram send error: {}",
                                        e
                                    ))
                                })?;
                            }
                            ErrorClass::RateLimited(secs) => {
                                if attempts > max_retries {
                                    return Err(ChannelError::RateLimited {
                                        retry_after_secs: secs,
                                    });
                                }
                                tracing::warn!(
                                    "Telegram rate limited, waiting {}s (attempt {}/{})",
                                    secs,
                                    attempts,
                                    max_retries
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    secs,
                                ))
                                .await;
                            }
                            ErrorClass::Recoverable => {
                                if attempts > max_retries {
                                    return Err(ChannelError::SendFailed(e.to_string()));
                                }
                                let backoff_ms = 500 * attempts as u64;
                                tracing::warn!(
                                    "Telegram send error (recoverable), retrying in {}ms (attempt {}/{}): {}",
                                    backoff_ms, attempts, max_retries, e
                                );
                                tokio::time::sleep(
                                    std::time::Duration::from_millis(backoff_ms),
                                )
                                .await;
                            }
                        }
                    }
                }
            };

            if is_first {
                first_msg = Some(sent);
            }
        }

        let sent = first_msg.expect("at least one chunk must be sent");

        // Send attachments if any
        for attachment in &message.attachments {
            self.send_attachment(bot, chat_id, thread_id, attachment).await?;
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

        let (chat_id, thread_id) = Self::parse_conversation_id(conversation_id.as_str());

        let mut req = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
        if let Some(tid) = thread_id {
            if tid != 1 {
                req = req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
            }
        }
        req.await
            .map_err(|e| ChannelError::Internal(format!("Failed to send typing: {}", e)))?;

        Ok(())
    }

    async fn react(&self, conversation_id: &ConversationId, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        let bot = self
            .bot
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Bot not initialized".to_string()))?;

        let (chat_id, _thread_id) = Self::parse_conversation_id(conversation_id.as_str());

        let msg_id = teloxide::types::MessageId(
            message_id
                .as_str()
                .parse::<i32>()
                .map_err(|e| ChannelError::Internal(format!("Invalid message ID: {}", e)))?,
        );

        let reactions = if reaction.is_empty() {
            vec![] // Remove reactions
        } else {
            vec![teloxide::types::ReactionType::Emoji {
                emoji: reaction.to_string(),
            }]
        };

        // Reactions are non-critical UX — swallow errors silently
        match bot.set_message_reaction(chat_id, msg_id).reaction(reactions).await {
            Ok(_) => {
                tracing::debug!("Reaction '{}' set on message {}", reaction, message_id.as_str());
                Ok(())
            }
            Err(e) => {
                tracing::debug!("Failed to set reaction (non-critical): {}", e);
                Ok(()) // Swallow — reactions are best-effort
            }
        }
    }

    async fn edit(&self, conversation_id: &ConversationId, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        self.edit_message(conversation_id, message_id, Some(new_text), None).await
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

    /// Send an attachment with optional forum-topic routing.
    async fn send_attachment(
        &self,
        bot: &Bot,
        chat_id: ChatId,
        thread_id: Option<i32>,
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

        /// Apply forum-topic thread ID to a teloxide request.
        macro_rules! with_thread {
            ($req:expr, $tid:expr) => {{
                let mut r = $req;
                if let Some(tid) = $tid {
                    if tid != 1 {
                        r = r.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
                    }
                }
                r
            }};
        }

        // Determine attachment type by MIME type
        let mime = &attachment.mime_type;
        if mime == "image/webp" || mime == "application/x-tgsticker" || mime == "video/webm" {
            // Sticker formats: static (webp), animated (tgsticker), video (webm)
            let req = with_thread!(bot.send_sticker(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send sticker: {}", e)))?;
        } else if mime.starts_with("image/") {
            let req = with_thread!(bot.send_photo(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send photo: {}", e)))?;
        } else if mime == "audio/ogg" || mime == "audio/opus" || mime == "audio/ogg; codecs=opus" {
            // Voice messages: OGG/Opus → send as voice (inline playable)
            let req = with_thread!(bot.send_voice(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send voice: {}", e)))?;
        } else if mime.starts_with("audio/") {
            // Other audio: MP3, WAV, etc. → also send as voice for TTS output
            let req = with_thread!(bot.send_voice(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send voice: {}", e)))?;
        } else if mime.starts_with("video/") {
            let req = with_thread!(bot.send_video(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send video: {}", e)))?;
        } else {
            let req = with_thread!(bot.send_document(chat_id, input_file), thread_id);
            req.await
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

        let (chat, _thread_id) = Self::parse_conversation_id(chat_id.as_str());

        let msg_id = teloxide::types::MessageId(message_id.as_str().parse().map_err(|_| {
            ChannelError::SendFailed("Invalid message ID".into())
        })?);

        if let Some(text) = new_text {
            // Convert Markdown to Telegram HTML for consistent rendering
            let html_text = MessageFormatter::format(text, MarkupFormat::TelegramHtml);

            // Edit text (and optionally keyboard)
            let mut request = bot.edit_message_text(chat, msg_id, &html_text)
                .parse_mode(ParseMode::Html);

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

    #[test]
    fn test_parse_conversation_id_plain() {
        let (chat_id, thread_id) = TelegramChannel::parse_conversation_id("-100123456789");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, None);
    }

    #[test]
    fn test_parse_conversation_id_with_topic() {
        let (chat_id, thread_id) = TelegramChannel::parse_conversation_id("-100123456789:topic:42");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(42));
    }

    #[test]
    fn test_parse_conversation_id_general_topic() {
        let (chat_id, thread_id) = TelegramChannel::parse_conversation_id("-100123456789:topic:1");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(1));
    }

    #[test]
    fn test_parse_conversation_id_invalid() {
        let (chat_id, thread_id) = TelegramChannel::parse_conversation_id("not_a_number");
        assert_eq!(chat_id.0, 0);
        assert_eq!(thread_id, None);
    }
}

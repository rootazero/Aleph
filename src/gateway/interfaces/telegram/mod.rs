//! Telegram Channel Implementation
//!
//! Integrates with the Telegram Bot API using the teloxide framework.
//!
//! # Features
//!
//! - Long-polling transport (teloxide dispatcher)
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
pub mod audit;
pub mod bot_instance;
pub mod chunking;
pub mod config;
pub mod config_resolver;
pub mod config_v2;
pub mod delivery;
pub mod error_cooldown;
pub mod handlers;
pub mod mention;
pub mod offset;
mod polling;
pub mod reaction_handler;
pub mod sticker;
pub mod streaming;
pub mod token_fingerprint;

pub use access::AccessController;
pub use bot_instance::BotInstance;
pub use config::{parse_telegram_channel_config, TelegramConfig};
pub use config_resolver::{ConfigResolver, ResolvedConfig};
pub use config_v2::TelegramConfigV2;
pub use config_v2::{DmPolicy, GroupPolicy, StatusReactionConfig, StreamingOptions};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId,
    MessageMeta, OutboundMessage, SendResult, UserId, CB_MESSAGE_ID_PREFIX,
};
use crate::sync_primitives::{Arc, Ordering};
use access::AccessDecision;
use async_trait::async_trait;
use chrono::Utc;
use error_cooldown::ErrorCooldown;
use tokio::sync::oneshot;

use teloxide::{prelude::*, types::CallbackQuery as TgCallbackQuery};

/// Telegram channel implementation
pub struct TelegramChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration (multi-account v2)
    config_v2: TelegramConfigV2,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Active bot instances (one per account)
    bot_instances: Vec<bot_instance::BotInstance>,
    /// `ToolCatalog` for building slash commands at startup
    tool_registry: Option<Arc<crate::tool_metadata::ToolCatalog>>,
    /// Per-account access controllers, keyed by `account_id` (SW-1: each
    /// account needs its own controller because they resolve against
    /// different parts of the `ConfigResolver`).
    access_by_account: std::collections::HashMap<String, Arc<AccessController>>,
    /// Per-account token-fingerprint verdicts from the last `start()`,
    /// surfaced to the doctor. The doctor reads this without touching the
    /// vault, so an operator can spot a misroute without restarting.
    token_fingerprint_verdicts:
        std::collections::HashMap<String, token_fingerprint::TokenFingerprintVerdict>,
    /// Channel-wide audit log (P2-C): one ring buffer per channel instance,
    /// consulted by the doctor to answer "what did this channel actually
    /// do in chat X". Restart clears it — the audit log is a debugging
    /// artefact, not a system of record (that is the agent-identity
    /// ledger's job).
    audit_log: Arc<audit::AuditLog>,
    /// Per-conversation error cooldown and typing circuit breaker.
    error_cooldown: Arc<ErrorCooldown>,
    /// Persistent polling offset tracker (set via `set_offset_tracker`).
    offset_tracker: Option<Arc<offset::OffsetTracker>>,
    /// State database for the sticker description cache (set via
    /// `set_state_database`). Pairing persistence now lives in the router's
    /// `pairing_store`.
    state_db: Option<Arc<crate::resilience::StateDatabase>>,
    /// Multi-account config resolver
    config_resolver: ConfigResolver,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(id: impl Into<String>, config_v2: TelegramConfigV2) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Telegram".to_string(),
            channel_type: "telegram".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        let resolver = std::sync::Arc::new(ConfigResolver::from_v2(&config_v2));
        // SW-1: each account gets its own controller. Most installs have a
        // single account; multi-account installs (rare) now resolve
        // independently.
        let mut access_by_account = std::collections::HashMap::new();
        if config_v2.accounts.is_empty() {
            // No accounts: still build a controller so the channel boots; it
            // returns Denied for everything.
            access_by_account.insert(
                "default".to_string(),
                Arc::new(AccessController::new(resolver.clone(), "default")),
            );
        } else {
            for account in &config_v2.accounts {
                access_by_account.insert(
                    account.id.clone(),
                    Arc::new(AccessController::new(resolver.clone(), &account.id)),
                );
            }
        }

        Self {
            info,
            config_v2,
            channel_state: ChannelState::new(100),
            bot_instances: Vec::new(),
            tool_registry: None,
            access_by_account,
            token_fingerprint_verdicts: std::collections::HashMap::new(),
            audit_log: Arc::new(audit::AuditLog::new()),
            error_cooldown: Arc::new(ErrorCooldown::new()),
            offset_tracker: None,
            state_db: None,
            config_resolver: (*resolver).clone(),
        }
    }

    /// Set the `ToolCatalog` so this channel can query builtin tools at startup
    /// and register them as Telegram slash commands.
    pub fn set_tool_registry(&mut self, registry: Arc<crate::tool_metadata::ToolCatalog>) {
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
    const fn capabilities() -> ChannelCapabilities {
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
            rich_text: true,
            // Telegram Bot API `sendPoll` (chat_id, question, options[2..10])
            // is stable since 2020. Wired in `create_poll` below.
            polls: true,
            group_icons: false, // Markdown/HTML support
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

impl TelegramChannel {
    /// Channel-wide audit log accessor (P2-C). Used by the doctor to
    /// answer "what did this Telegram channel actually do" without
    /// needing to scrape logs. Not part of the `Channel` trait — this is
    /// a Telegram-specific diagnostic that other channels may grow their
    /// own equivalent for.
    #[must_use]
    pub fn audit_log(&self) -> &Arc<audit::AuditLog> {
        &self.audit_log
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

    // `get_pairing_data` / `list_active_pairing_codes` intentionally fall back to
    // the `Channel` trait defaults (no local pairing). Pairing is owned by the
    // inbound router's `pairing_store`: an unpaired DM is auto-issued a code and
    // approved by the operator via `pairing.approve` (see the telegram:access
    // skill), so the channel no longer mints its own codes.

    async fn start(&mut self) -> ChannelResult<()> {
        if self.config_v2.accounts.is_empty() {
            return Err(ChannelError::ConfigError(
                "No Telegram accounts configured".to_string(),
            ));
        }

        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!("Starting Telegram channel...");

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
                    html_fallback: account.html_fallback.unwrap_or(true),
                    link_preview: account.link_preview.unwrap_or_default(),
                });

            let account_access = self.access_by_account.get(&account.id).cloned().expect(
                "start() runs after new() which inserts every account's \
                     controller into access_by_account",
            );
            let mut instance = BotInstance::new(account, resolved_config, account_access.clone());

            // Group mention gate: only respond to addressed group messages when
            // the account opts in. The bot's live `@username` (authoritative,
            // from `get_me` below) is what the gate matches against.
            let require_mention = account.require_mention.unwrap_or(false);
            let bot_username_resolved: Option<String>;

            // P0-C: token-fingerprint check. Opt-in via the config field; runs
            // BEFORE `get_me()` so a wrong token is observable in the same
            // diagnostic block as the connect attempt, not after a flurry of
            // unrelated warnings. A mismatch is logged + remembered in the
            // channel-level list for the doctor; the channel still starts so
            // the operator gets a useful diagnostic, not a refuse-to-start
            // that hides the actual misroute.
            let verdict = token_fingerprint::TokenFingerprintVerdict::verify(
                &account.id,
                &account.bot_token,
                account.token_fingerprint.as_deref(),
            );
            match &verdict {
                token_fingerprint::TokenFingerprintVerdict::NotConfigured => {}
                token_fingerprint::TokenFingerprintVerdict::Match {
                    fingerprint_prefix, ..
                } => {
                    tracing::info!(
                        account_id = %account.id,
                        fingerprint_prefix = %fingerprint_prefix,
                        "Telegram token fingerprint matches configured value"
                    );
                }
                token_fingerprint::TokenFingerprintVerdict::Mismatch {
                    expected_prefix,
                    actual_prefix,
                    ..
                } => {
                    tracing::warn!(
                        account_id = %account.id,
                        expected_prefix = %expected_prefix,
                        actual_prefix = %actual_prefix,
                        "Telegram token fingerprint MISMATCH — the running bot_token does \
                         not match the configured token_fingerprint; outbound messages will \
                         hit the wrong account. Set/clear token_fingerprint or restore the \
                         correct bot_token."
                    );
                    self.token_fingerprint_verdicts
                        .insert(account.id.clone(), verdict.clone());
                }
            }

            // Verify bot token by getting bot info
            match instance.bot.get_me().await {
                Ok(me) => {
                    // Capture the live handle for the mention gate.
                    bot_username_resolved = Some(me.username().to_string());
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
                                        warnings.push(format!("Group {gid} unreachable: {e}"));
                                    }
                                    Err(_) => {
                                        warnings.push(format!("Group {gid} check timed out"));
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

            // Build slash commands from ToolCatalog (user-facing commands only)
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
                            format!("{truncated}...")
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
            let channel_id = self.info.id.clone();
            let channel_id_for_cb = self.info.id.clone();

            let access_clone = account_access.clone();
            let access_for_cb = account_access;

            let state_db_for_sticker = self.state_db.clone();

            // Captured by the per-update message handler for group mention gating.
            let mention_username_cap = bot_username_resolved.clone();
            let require_mention_cap = require_mention;

            let message_handler = Update::filter_message().endpoint(
                move |bot: Bot, msg: teloxide::types::Message| {
                    let inbound_tx = inbound_tx.clone();
                    let channel_id = channel_id.clone();
                    let access = access_clone.clone();
                    let mention_username = mention_username_cap.clone();
                    let require_mention = require_mention_cap;
                    let sticker_pipeline =
                        sticker::StickerPipeline::new(state_db_for_sticker.clone());
                    async move {
                        let user_id = msg.from.as_ref().map_or(0, |u| u.id.0 as i64);
                        let is_group = msg.chat.is_group() || msg.chat.is_supergroup();
                        let chat_id = msg.chat.id.0;
                        // Forum-topic id (or None for non-topic messages). The
                        // inbound context already encodes topic into
                        // conversation_id; we still need the raw thread_id here
                        // so the access controller's resolver lookup actually
                        // hits the per-topic override (SW-1).
                        let thread_id_i32: Option<i32> = msg.thread_id.map(|t| t.0 .0);

                        // Group mention gate (pure, deterministic I/O filter — R4).
                        // Drops ambient group chatter that does not address the
                        // bot when `require_mention` is enabled. DMs bypass this.
                        //
                        // `group_addresses_bot` is computed for every group
                        // message (not just under `require_mention`) because it
                        // also drives the `AppMention` tag below: the limb has
                        // the bot's real @username, reply-to-bot signal and
                        // command syntax, so when it affirmatively recognises an
                        // address we tag the message and the central router skips
                        // its weaker substring check instead of re-dropping it.
                        let group_addresses_bot = if is_group {
                            let bot_uname = mention_username.as_deref();
                            let reply_to_bot = msg
                                .reply_to_message()
                                .and_then(|r| r.from.as_ref())
                                .is_some_and(|u| {
                                    u.is_bot
                                        && match (bot_uname, u.username.as_deref()) {
                                            (Some(b), Some(ru)) => b.eq_ignore_ascii_case(ru),
                                            _ => false,
                                        }
                                });
                            let text = msg.text().or_else(|| msg.caption());
                            mention::group_message_addresses_bot(text, reply_to_bot, bot_uname)
                        } else {
                            false
                        };
                        if is_group && require_mention && !group_addresses_bot {
                            tracing::debug!(
                                channel = "telegram",
                                chat_id = %chat_id,
                                "group message not addressed to bot — skipped (require_mention)"
                            );
                            return Ok::<(), std::convert::Infallible>(());
                        }

                        match access.check_message(user_id, chat_id, thread_id_i32, is_group) {
                            AccessDecision::Allowed => {
                                if let Some(mut inbound) = handlers::convert_message(
                                    &msg,
                                    &bot,
                                    &channel_id,
                                    &sticker_pipeline,
                                )
                                .await
                                {
                                    // Limb-validated address (real @username,
                                    // /cmd@bot, or reply-to-bot): tag so the
                                    // central inbound router bypasses its crude
                                    // substring mention check, which would
                                    // otherwise re-drop a reply-to-bot message or
                                    // a mention of a bot not named "aleph".
                                    if group_addresses_bot {
                                        inbound.metadata.push(MessageMeta::AppMention);
                                    }

                                    if let Err(e) = inbound_tx.send(inbound) {
                                        tracing::error!("Failed to send inbound message: {:?}", e);
                                    }
                                }
                            }
                            AccessDecision::NeedsPairing => {
                                // The inbound router owns the authoritative pairing
                                // gate (`pairing_store` + `check_permission`): an
                                // unpaired DM is denied there and a pairing request
                                // is minted/sent. Forwarding here lets the router
                                // run that single-source flow; the local access
                                // controller only pre-classifies.
                                if let Some(inbound) = handlers::convert_message(
                                    &msg,
                                    &bot,
                                    &channel_id,
                                    &sticker_pipeline,
                                )
                                .await
                                {
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
                },
            );

            let callback_handler =
                Update::filter_callback_query().endpoint(move |bot: Bot, q: TgCallbackQuery| {
                    let inbound_tx = inbound_tx_for_cb.clone();
                    let channel_id = channel_id_for_cb.clone();
                    let access = access_for_cb.clone();
                    async move {
                        let (raw_chat_id, thread_id_val) =
                            q.message.as_ref().map_or((0, None), |m| {
                                let chat = m.chat().id.0;
                                let tid = match m {
                                    teloxide::types::MaybeInaccessibleMessage::Regular(msg) => {
                                        msg.thread_id.map(|t| t.0 .0)
                                    }
                                    _ => None,
                                };
                                (chat, tid)
                            });

                        let conv_id_str = if let Some(tid) = thread_id_val {
                            format!("{raw_chat_id}:topic:{tid}")
                        } else {
                            raw_chat_id.to_string()
                        };

                        if let Some(data) = q.data.clone() {
                            let user_id_val = q.from.id.0 as i64;

                            let is_group = raw_chat_id < 0;
                            let decision = access.check_message(
                                user_id_val,
                                raw_chat_id,
                                thread_id_val,
                                is_group,
                            );
                            if decision == AccessDecision::Allowed {
                                let inbound = InboundMessage {
                                    id: MessageId::new(format!("{CB_MESSAGE_ID_PREFIX}{}", q.id)),
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
                                        "Failed to send callback as inbound message: {:?}",
                                        e
                                    );
                                }
                            }
                        }

                        if let Err(e) = bot.answer_callback_query(q.id).await {
                            tracing::warn!("Failed to answer callback query: {}", e);
                        }

                        Ok::<(), std::convert::Infallible>(())
                    }
                });

            let inbound_tx_for_poll = self.channel_state.sender();
            let channel_id_for_poll = self.info.id.clone();
            let poll_handler =
                Update::filter_poll_answer().endpoint(move |q: teloxide::types::PollAnswer| {
                    let inbound_tx = inbound_tx_for_poll.clone();
                    let channel_id = channel_id_for_poll.clone();
                    async move {
                        let (conversation_id, sender_id, sender_name) = match &q.voter {
                            teloxide::types::MaybeAnonymousUser::User(user) => (
                                ConversationId::new(user.id.to_string()),
                                UserId::new(user.id.to_string()),
                                user.username
                                    .clone()
                                    .or_else(|| Some(user.first_name.clone())),
                            ),
                            teloxide::types::MaybeAnonymousUser::Chat(chat) => (
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
                                poll_id: q.poll_id.to_string(),
                                option_ids: q.option_ids,
                            }],
                        };
                        let _ = inbound_tx.send(inbound);
                        Ok::<(), std::convert::Infallible>(())
                    }
                });

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

            // Spawn periodic health check for this bot instance
            {
                let account_id = instance.account_id.clone();
                let bot = instance.bot.clone();
                let is_healthy = instance.is_healthy.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        match tokio::time::timeout(std::time::Duration::from_secs(10), bot.get_me())
                            .await
                        {
                            Ok(Ok(_)) => {
                                is_healthy.store(true, Ordering::Relaxed);
                            }
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    account_id = %account_id,
                                    error = %e,
                                    "Telegram bot health check failed"
                                );
                                is_healthy.store(false, Ordering::Relaxed);
                            }
                            Err(_) => {
                                tracing::warn!(
                                    account_id = %account_id,
                                    "Telegram bot health check timed out"
                                );
                                is_healthy.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                });
            }

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
        let (chat_id, thread_id) =
            delivery::parse_conversation_id(message.conversation_id.as_str())?;
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No Telegram account configured for chat {chat_id_i64} (thread: {thread_id:?}). \
                     Ensure the chat_id is covered by allowed_groups or account config"
                ))
            })?;
        let result = delivery::send_message(
            &instance.bot,
            &instance.resolved_config,
            &message,
            &self.error_cooldown,
        )
        .await;
        // P2-C: write the audit row regardless of outcome. A failed send
        // is the most useful kind to record (the doctor can answer "why
        // did chat X stop responding" without scraping logs). The
        // audit_log.push is best-effort and never panics.
        let outcome = match &result {
            Ok(r) => format!("succeeded: msg_id={}", r.message_id.as_str()),
            Err(e) => format!("failed: {e}"),
        };
        self.audit_log.push(audit::entry(
            instance.account_id.clone(),
            Some(chat_id_i64),
            thread_id,
            audit::AuditKind::SendAttempted {
                message_id_kind: message
                    .text
                    .chars()
                    .take(64)
                    .collect::<String>()
                    .replace('\n', " "),
                outcome,
            },
        ));
        result
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str())?;
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No Telegram account configured for chat {chat_id_i64} (thread: {thread_id:?}). \
                     Ensure the chat_id is covered by allowed_groups or account config"
                ))
            })?;
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
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str())?;
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No Telegram account configured for chat {chat_id_i64} (thread: {thread_id:?}). \
                     Ensure the chat_id is covered by allowed_groups or account config"
                ))
            })?;
        delivery::send_reaction(
            &instance.bot,
            conversation_id.as_str(),
            message_id,
            reaction,
        )
        .await
    }

    async fn create_poll(
        &self,
        conversation_id: &ConversationId,
        question: &str,
        options: &[String],
        allow_multiple: bool,
    ) -> ChannelResult<MessageId> {
        // Telegram's `sendPoll` rejects <2 or >10 options (validation done by
        // the bot API itself). The Channel trait's `create_poll` default does
        // not pre-validate; we mirror the bot API floor so the error message
        // names the constraint instead of a generic 400.
        if options.len() < 2 {
            return Err(ChannelError::SendFailed(
                "create_poll requires at least 2 options".into(),
            ));
        }
        if options.len() > 10 {
            return Err(ChannelError::SendFailed(
                "create_poll accepts at most 10 options".into(),
            ));
        }

        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str())?;
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No Telegram account configured for chat {chat_id_i64} (thread: {thread_id:?}). \
                     Ensure the chat_id is covered by allowed_groups or account config"
                ))
            })?;

        // `send_poll` returns a `Message` carrying the poll id; we surface
        // the message id (the Channel trait only has MessageId — downstream
        // callers that want the poll id can call `bot.get_chat(chat_id)` and
        // inspect, or open a follow-up that extends the trait).
        let opts: Vec<teloxide::types::InputPollOption> = options
            .iter()
            .cloned()
            .map(teloxide::types::InputPollOption::from)
            .collect();
        let mut req = instance.bot.send_poll(chat_id, question, opts);
        if allow_multiple {
            req = req.allows_multiple_answers(true);
        }
        let msg = req
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("sendPoll failed: {e}")))?;
        Ok(MessageId::new(msg.id.0.to_string()))
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let (chat_id, thread_id) = delivery::parse_conversation_id(conversation_id.as_str())?;
        let chat_id_i64 = chat_id.0;

        let instance = self
            .bot_instances
            .iter()
            .find(|inst| {
                self.config_resolver
                    .resolve(&inst.account_id, chat_id_i64, thread_id)
                    .is_some()
            })
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No Telegram account configured for chat {chat_id_i64} (thread: {thread_id:?}). \
                     Ensure the chat_id is covered by allowed_groups or account config"
                ))
            })?;
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
            access: first.access.clone(),
            offset_tracker: first.offset_tracker.clone(),
            shutdown_tx: None,
            is_healthy: first.is_healthy.clone(),
        };
        let first_access = first.access.clone();
        Some(Arc::new(
            crate::gateway::interfaces::telegram::approval::TelegramChannelApprovalCapability::new(
                Arc::new(Self {
                    info: self.info.clone(),
                    config_v2: self.config_v2.clone(),
                    channel_state: ChannelState::new(100),
                    bot_instances: vec![instance],
                    tool_registry: self.tool_registry.clone(),
                    access_by_account: std::collections::HashMap::from([(
                        first.account_id.clone(),
                        first_access.clone(),
                    )]),
                    token_fingerprint_verdicts: self.token_fingerprint_verdicts.clone(),
                    audit_log: self.audit_log.clone(),
                    error_cooldown: self.error_cooldown.clone(),
                    offset_tracker: self.offset_tracker.clone(),
                    state_db: self.state_db.clone(),
                    config_resolver: self.config_resolver.clone(),
                }),
                first_access,
            ),
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
        let config = config::parse_telegram_channel_config(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Telegram config: {e}")))?;

        Ok(Box::new(TelegramChannel::new("telegram", config)))
    }
}

fn telegram_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> crate::gateway::channel::ChannelResult<crate::sync_primitives::Arc<dyn ChannelFactory>> {
    Ok(crate::sync_primitives::Arc::new(TelegramChannelFactory))
}

pub fn register_with_plugin() {
    let _ = crate::gateway::interfaces::plugin::register("telegram", telegram_factory_creator);
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
        assert!(
            caps.polls,
            "Telegram has stable sendPoll — wire must reflect it"
        );
        assert_eq!(caps.max_message_length, 4096);
    }

    #[tokio::test]
    async fn create_poll_enforces_telegram_option_floor() {
        // Telegram's bot API rejects <2 / >10 options. The Channel impl
        // pre-validates so the error names the constraint (the bot API
        // would surface a generic 400). Pin the floor here so a future
        // edit cannot silently lower the gate and let an empty poll
        // reach teloxide.
        let config = TelegramConfigV2 {
            accounts: vec![
                crate::gateway::interfaces::telegram::config_v2::TelegramAccountConfig {
                    id: "default".to_string(),
                    bot_token: "123:ABC".to_string(),
                    token_fingerprint: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let channel = TelegramChannel::new("telegram-test", config);

        let cid = ConversationId::new("123456");
        // 1 option -> floor violation, no bot instance required because the
        // pre-validation runs first.
        let err = channel
            .create_poll(&cid, "Lunch?", &["OnlyOne".into()], false)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("at least 2"),
            "expected floor violation, got: {err}"
        );
        // 11 options -> ceiling violation.
        let many: Vec<String> = (0..11).map(|i| format!("opt{i}")).collect();
        let err = channel
            .create_poll(&cid, "Lunch?", &many, false)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("at most 10"),
            "expected ceiling violation, got: {err}"
        );
    }

    #[test]
    fn test_channel_creation() {
        let config = TelegramConfigV2 {
            accounts: vec![
                crate::gateway::interfaces::telegram::config_v2::TelegramAccountConfig {
                    id: "default".to_string(),
                    bot_token: "123:ABC".to_string(),
                    token_fingerprint: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let channel = TelegramChannel::new("telegram-test", config);
        assert_eq!(channel.info().id.as_str(), "telegram-test");
        assert_eq!(channel.info().channel_type, "telegram");
    }
}

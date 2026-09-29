//! Telegram message delivery — send, retry, chunking, attachments.
//!
//! All outbound message operations extracted from the Channel trait impl.
//! Each function takes a `&Bot` directly (no `&self`), making them
//! independently testable and reusable from both Channel and `MessageOps`.

use crate::gateway::channel::{
    Attachment, ChannelError, ChannelResult, InlineKeyboard, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use teloxide::{
    prelude::*,
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode, ThreadId},
};

use super::chunking::split_html_safe;
use super::config_resolver::ResolvedConfig;
use super::config_v2::LinkPreviewMode;
use super::error_cooldown::{ErrorCooldown, ErrorKind};

/// Map the configured link-preview policy to teloxide's `LinkPreviewOptions`.
///
/// Returns `None` for [`LinkPreviewMode::Enabled`] — Telegram's default
/// behaviour, so the request is left untouched (no API field emitted).
const fn link_preview_options(
    mode: LinkPreviewMode,
) -> Option<teloxide::types::LinkPreviewOptions> {
    use teloxide::types::LinkPreviewOptions;
    match mode {
        LinkPreviewMode::Enabled => None,
        LinkPreviewMode::Disabled => Some(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        }),
        LinkPreviewMode::Above => Some(LinkPreviewOptions {
            is_disabled: false,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: true,
        }),
    }
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Classification of Telegram API errors for retry logic.
///
/// `#[doc(hidden)]` — exposed alongside `classify_error` for tests; not
/// part of Aleph's stable API. The variants are still crate-internal
/// (only `classify_error` returns them) but the type's visibility has to
/// match the function's.
#[derive(Debug)]
#[doc(hidden)]
pub enum ErrorClass {
    /// Timeout/reset — may have been sent, retry cautiously.
    PostConnect,
    /// Network-class error with finer granularity than just "PreConnect /
    /// PostConnect": the retry budget, log level, and operator diagnostics
    /// all change with DNS vs TLS vs connect-timeout. Each variant is still
    /// `Retryable` for the cooldown book — the distinction is for telemetry,
    /// not retry policy.
    Network(NetworkKind),
    /// Telegram API rejection — don't retry, fallback to plain text.
    Rejected(String),
    /// 429 rate limit — wait exact seconds then retry.
    RateLimited(u64),
    /// HTML parse error — fallback to plain text if enabled.
    HtmlParseError(String),
    /// The bot lost authority to act in the target chat. **Permanent** for
    /// that specific chat, but a DIFFERENT chat on the same account is
    /// unaffected — so the cooldown is keyed on `(chat_id, kind)`, not the
    /// whole account. Splits what was previously lumped into `Rejected` so
    /// the operator's diagnostics can distinguish "the user blocked the bot"
    /// (resumable when they unblock) from "the chat was deleted" (gone
    /// forever).
    Forbidden(ForbiddenKind),
}

/// Network error sub-kind for diagnostics. Doesn't change retry policy
/// (`ErrorClass` still maps to `ErrorKind::Retryable`), but the operator's
/// log + doctor dashboard do show the distinction — DNS failure on the bot's
/// host means "your resolver is broken", TLS handshake means "your proxy /
/// cert is broken", connect timeout means "the route is congested".
///
/// `#[doc(hidden)]` — see `ErrorClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum NetworkKind {
    Dns,
    Tls,
    Connect,
    Timeout,
}

/// Sub-kind for `ErrorClass::Forbidden`. Distinct because each one carries a
/// different unblock path:
/// - `BotBlocked`: the user explicitly blocked this bot; unblockable from the
///   bot's side. Recovery is the user un-blocking.
/// - `UserNotFound`: the recipient's user_id no longer resolves; only fixed by
///   a fresh user list (the user may have re-registered under a new id).
/// - `ChatNotFound`: the chat was deleted or the bot was kicked. Permanent.
/// - `MessageNotFound`: the bot tried to edit / delete a message that is
///   already gone (user deleted it mid-stream). Different shape from the
///   above three: it is NOT a "this chat is dead" signal, it's a "this
///   particular edit target is gone" signal, and the cooldown key must be the
///   message id, not the chat — otherwise editing a missing message would
///   park the whole conversation for hours.
///
/// `#[doc(hidden)]` — see `ErrorClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum ForbiddenKind {
    BotBlocked,
    UserNotFound,
    ChatNotFound,
    MessageNotFound,
}

/// Classify a teloxide request error for retry decisions.
///
/// Uses teloxide's typed enums (`ApiError`, `RequestError::RetryAfter`) and
/// reqwest's `is_connect()` for precise classification instead of fragile
/// string matching.
///
/// `#[doc(hidden)]` — exposed for `tests/telegram_delivery_e2e.rs` so the
/// P0-C error taxonomy (Network/Forbidden vs Rejected) can be asserted
/// directly without round-tripping through the network.
#[doc(hidden)]
pub fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        // Rate limit is a top-level RequestError variant (not inside ApiError)
        teloxide::RequestError::RetryAfter(seconds) => {
            ErrorClass::RateLimited(u64::from(seconds.seconds()))
        }
        teloxide::RequestError::Api(api_err) => {
            use teloxide::ApiError;
            match api_err {
                // Forbidden bucket: chat/user/message-level permission or
                // existence problems. They look similar (they're all "the
                // server says you can't") but they have DIFFERENT recovery
                // semantics (see ForbiddenKind doc), so they need to be
                // distinguishable in the operator's log / doctor.
                ApiError::BotBlocked => ErrorClass::Forbidden(ForbiddenKind::BotBlocked),
                ApiError::ChatNotFound => ErrorClass::Forbidden(ForbiddenKind::ChatNotFound),
                ApiError::UserNotFound => ErrorClass::Forbidden(ForbiddenKind::UserNotFound),
                // Edit / delete targets that vanished mid-stream: NOT a chat-
                // level permanent. Previously lumped into `Rejected` here, which
                // parked the conversation for 4 hours over a missing edit
                // target — exactly the bug class the new bucket prevents.
                ApiError::MessageNotModified
                | ApiError::MessageCantBeEdited
                | ApiError::MessageToEditNotFound
                | ApiError::MessageToDeleteNotFound => {
                    ErrorClass::Forbidden(ForbiddenKind::MessageNotFound)
                }
                // Invalid token is permanent
                ApiError::InvalidToken => ErrorClass::Rejected(api_err.to_string()),
                // Catch other permanent errors by message content
                _ => {
                    let msg = api_err.to_string();
                    if msg.contains("Bad Request") {
                        // Distinguish HTML parse errors for fallback logic
                        if msg.contains("can't parse entities")
                            || msg.contains("parse entities")
                            || msg.contains("find end of the entity")
                            || msg.contains("message text is empty")
                        {
                            ErrorClass::HtmlParseError(msg)
                        } else {
                            ErrorClass::Rejected(msg)
                        }
                    } else {
                        ErrorClass::PostConnect
                    }
                }
            }
        }
        teloxide::RequestError::Network(reqwest_err) => {
            // Split the old PreConnect / PostConnect bucket by ROOT CAUSE so
            // the operator's diagnostic path points at the right subsystem.
            // DNS failure means "the host's resolver is broken", TLS means
            // "the cert chain or the proxy TLS config is broken", connect
            // timeout means "the route is congested / firewalled", and a
            // request-body timeout is the "we got TCP but Telegram stopped
            // mid-stream" case — all of these would have been reported as
            // the same indistinguishable "PreConnect" before.
            if reqwest_err.is_connect() {
                if reqwest_err.is_timeout() {
                    ErrorClass::Network(NetworkKind::Timeout)
                } else if reqwest_err.to_string().to_lowercase().contains("dns")
                    || reqwest_err.to_string().to_lowercase().contains("resolve")
                {
                    ErrorClass::Network(NetworkKind::Dns)
                } else if reqwest_err.to_string().to_lowercase().contains("tls")
                    || reqwest_err.to_string().to_lowercase().contains("handshake")
                    || reqwest_err
                        .to_string()
                        .to_lowercase()
                        .contains("certificate")
                {
                    ErrorClass::Network(NetworkKind::Tls)
                } else {
                    ErrorClass::Network(NetworkKind::Connect)
                }
            } else if reqwest_err.is_timeout() {
                ErrorClass::Network(NetworkKind::Timeout)
            } else {
                ErrorClass::PostConnect // reset, body read failure, etc.
            }
        }
        _ => ErrorClass::PostConnect,
    }
}

/// Map an `ErrorClass` to an `ErrorKind` for cooldown purposes.
const fn error_class_to_kind(ec: &ErrorClass) -> ErrorKind {
    match ec {
        // Forbidden splits:
        //   - `BotBlocked` / `UserNotFound` / `ChatNotFound`: PERMANENT for
        //     the chat. Cooldown goes 4 h, same as the old `Rejected` bucket.
        //   - `MessageNotFound`: NOT a chat-level permanent. The conversation
        //     may still be perfectly healthy — only this specific edit /
        //     delete target is gone. Mapped to `Retryable` so a missing
        //     target message on one edit does NOT park the conversation.
        ErrorClass::Forbidden(kind) => match kind {
            ForbiddenKind::BotBlocked
            | ForbiddenKind::UserNotFound
            | ForbiddenKind::ChatNotFound => ErrorKind::Permanent,
            ForbiddenKind::MessageNotFound => ErrorKind::Retryable,
        },
        ErrorClass::Rejected(_) | ErrorClass::HtmlParseError(_) => ErrorKind::Permanent,
        ErrorClass::PostConnect | ErrorClass::Network(_) | ErrorClass::RateLimited(_) => {
            ErrorKind::Retryable
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a `conversation_id` that may contain a forum topic suffix.
///
/// Format: `"{chat_id}"` or `"{chat_id}:topic:{thread_id}"`.
/// Returns the `ChatId` and an optional raw thread id (i32).
pub(crate) fn parse_conversation_id(conv_id: &str) -> ChannelResult<(ChatId, Option<i32>)> {
    if let Some((chat, topic)) = conv_id.split_once(":topic:") {
        let chat_id = chat.parse::<i64>().map_err(|_| {
            ChannelError::Internal(format!("Invalid chat_id in conversation_id: {conv_id}"))
        })?;
        let thread_id = topic.parse::<i32>().map_err(|_| {
            ChannelError::Internal(format!("Invalid thread_id in conversation_id: {conv_id}"))
        })?;
        Ok((ChatId(chat_id), Some(thread_id)))
    } else {
        let chat_id = conv_id
            .parse::<i64>()
            .map_err(|_| ChannelError::Internal(format!("Invalid conversation_id: {conv_id}")))?;
        Ok((ChatId(chat_id), None))
    }
}

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

// ---------------------------------------------------------------------------
// Benign edit errors (streaming)
// ---------------------------------------------------------------------------

/// Returns `true` for Telegram API errors that are expected and non-fatal
/// during edit-based streaming delivery.
///
/// - `MessageNotModified`: debounce window produced no new tokens.
/// - `MessageCantBeEdited`: user deleted the message mid-stream.
const fn is_benign_edit_error(err: &teloxide::RequestError) -> bool {
    matches!(
        err,
        teloxide::RequestError::Api(
            teloxide::ApiError::MessageNotModified
                | teloxide::ApiError::MessageCantBeEdited
                | teloxide::ApiError::EditedMessageIsTooLong
        )
    )
}

// ---------------------------------------------------------------------------
// Core send
// ---------------------------------------------------------------------------

/// Send an outbound message with chunking, retry, and attachment support.
///
/// This is the extracted body of `Channel::send()`.
///
/// `#[doc(hidden)]` — exposed for `tests/telegram_delivery_e2e.rs`, which
/// drives this through a wiremock-backed `Bot::set_api_url` to exercise the
/// error-taxonomy + media-group + voice-note paths without a real Telegram
/// round-trip. The function is *not* part of Aleph's stable API; callers
/// outside this crate should go through `Channel::send`.
#[doc(hidden)]
pub async fn send_message(
    bot: &Bot,
    config: &ResolvedConfig,
    message: &OutboundMessage,
    cooldown: &ErrorCooldown,
) -> ChannelResult<SendResult> {
    let conv_id = message.conversation_id.as_str();
    let (chat_id, thread_id) = parse_conversation_id(conv_id)?;

    // Check cooldown before attempting to send
    if let Err(cd) = cooldown.check(conv_id) {
        tracing::warn!(
            conversation_id = conv_id,
            remaining_secs = cd.remaining.as_secs_f64(),
            "skipping send — conversation in cooldown",
        );
        let err = ChannelError::SendFailed(format!("conversation in cooldown: {cd}"));
        if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
            return Err(ChannelError::SendFailed(
                "Error suppressed by policy".to_string(),
            ));
        }
        return Err(err);
    }

    // Send typing indicator if enabled and typing breaker allows it
    if config.send_typing && cooldown.check_typing() {
        let mut action_req = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
        if let Some(tid) = thread_id {
            // Unlike `sendMessage` (which must OMIT message_thread_id for the
            // General topic, tid == 1, or Telegram rejects with "thread not
            // found"), `sendChatAction` must always INCLUDE it — otherwise the
            // typing indicator never appears in that topic. openclaw parity:
            // the General-topic send-vs-typing asymmetry.
            action_req = action_req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
        }
        match action_req.await {
            Ok(_) => cooldown.record_typing_success(),
            Err(_) => cooldown.record_typing_failure(),
        }
    }

    // Voice-only: if text is empty but attachments exist, skip text and send attachments only
    if message.text.is_empty() && !message.attachments.is_empty() {
        let mut first_msg_id = None;
        for attachment in &message.attachments {
            let result = send_attachment(bot, chat_id, thread_id, attachment).await;
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

    // Convert to HTML first, then split with HTML-aware chunking.
    // Telegram's limit is 4096 chars; we use 4000 to leave a small margin.
    let html_text = MessageFormatter::format(&message.text, MarkupFormat::TelegramHtml);
    let chunks = split_html_safe(&html_text, 4000);

    // Helper to build a SendMessage request with optional thread routing
    let build_request = |parse_mode: Option<ParseMode>,
                         text: &str,
                         reply_to: Option<&str>,
                         keyboard: Option<&InlineKeyboard>| {
        let mut req = bot.send_message(chat_id, text);
        if let Some(mode) = parse_mode {
            req = req.parse_mode(mode);
        }
        if let Some(opts) = link_preview_options(config.link_preview) {
            req = req.link_preview_options(opts);
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
                req = req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
            }
        }
        if let Some(keyboard) = keyboard {
            let markup = InlineKeyboardMarkup::new(keyboard.rows.iter().map(|row| {
                row.iter()
                    .map(|btn| InlineKeyboardButton::callback(&btn.text, &btn.callback_data))
                    .collect::<Vec<_>>()
            }));
            req = req.reply_markup(markup);
        }
        req
    };

    // Send reasoning first if present in metadata
    if let Some(reasoning) = message.metadata.get("reasoning") {
        if !reasoning.is_empty() {
            let reasoning_text = format!("🤔 {reasoning}");
            let reasoning_html =
                MessageFormatter::format(&reasoning_text, MarkupFormat::TelegramHtml);
            let reasoning_chunks = split_html_safe(&reasoning_html, 4000);
            for chunk in &reasoning_chunks {
                let req = build_request(Some(ParseMode::Html), chunk, None, None);
                if let Err(e) = req.await {
                    tracing::warn!("Failed to send reasoning message: {}", e);
                    // Non-fatal: continue to main message
                }
            }
        }
    }

    // Send each chunk with retry logic. Only the first chunk carries
    // reply_to and inline_keyboard; subsequent chunks are plain continuations.
    let max_retries = config.max_retries;
    let mut first_msg: Option<teloxide::types::Message> = None;

    for (i, chunk) in chunks.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == chunks.len() - 1;
        // Chunks are already HTML-formatted with balanced tags
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
            let result =
                build_request(Some(ParseMode::Html), chunk, reply_to_ref, keyboard_ref).await;
            match result {
                Ok(msg) => break msg,
                Err(e) => {
                    attempts += 1;
                    match classify_error(&e) {
                        ErrorClass::Rejected(reason) => {
                            // Permanent rejection — try plain text fallback
                            tracing::warn!(
                                "HTML send rejected ({}), falling back to plain text",
                                reason
                            );
                            match build_request(None, chunk, reply_to_ref, keyboard_ref).await {
                                Ok(msg) => break msg,
                                Err(fallback_err) => {
                                    let cls = classify_error(&fallback_err);
                                    cooldown.record_failure(conv_id, error_class_to_kind(&cls));
                                    let err = ChannelError::SendFailed(format!(
                                        "Telegram send error: {fallback_err}"
                                    ));
                                    if !cooldown.should_send_error(
                                        conv_id,
                                        &config.error_policy,
                                        "",
                                    ) {
                                        return Err(ChannelError::SendFailed(
                                            "Error suppressed by policy".to_string(),
                                        ));
                                    }
                                    return Err(err);
                                }
                            }
                        }
                        ErrorClass::HtmlParseError(reason) => {
                            if config.html_fallback {
                                tracing::warn!(
                                    "HTML parse error ({}), falling back to plain text",
                                    reason
                                );
                                match build_request(None, chunk, reply_to_ref, keyboard_ref).await {
                                    Ok(msg) => break msg,
                                    Err(fallback_err) => {
                                        let cls = classify_error(&fallback_err);
                                        cooldown.record_failure(conv_id, error_class_to_kind(&cls));
                                        let err = ChannelError::SendFailed(format!(
                                            "Telegram send error: {fallback_err}"
                                        ));
                                        if !cooldown.should_send_error(
                                            conv_id,
                                            &config.error_policy,
                                            "",
                                        ) {
                                            return Err(ChannelError::SendFailed(
                                                "Error suppressed by policy".to_string(),
                                            ));
                                        }
                                        return Err(err);
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "HTML parse error ({}) — html_fallback disabled, returning error",
                                    reason
                                );
                                cooldown.record_failure(conv_id, ErrorKind::Permanent);
                                if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                                    return Err(ChannelError::SendFailed(
                                        "Error suppressed by policy".to_string(),
                                    ));
                                }
                                return Err(ChannelError::SendFailed(format!(
                                    "HTML parse error: {reason}"
                                )));
                            }
                        }
                        ErrorClass::RateLimited(secs) => {
                            if attempts > max_retries {
                                cooldown.record_failure(conv_id, ErrorKind::Retryable);
                                if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                                    return Err(ChannelError::SendFailed(
                                        "Error suppressed by policy".to_string(),
                                    ));
                                }
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
                            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                        }
                        ErrorClass::PostConnect => {
                            // Data may have been sent — limit retries to avoid duplicates
                            let post_connect_max = max_retries.min(2);
                            if attempts > post_connect_max {
                                cooldown.record_failure(conv_id, ErrorKind::Retryable);
                                if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                                    return Err(ChannelError::SendFailed(
                                        "Error suppressed by policy".to_string(),
                                    ));
                                }
                                return Err(ChannelError::SendFailed(e.to_string()));
                            }
                            let backoff_ms = 1000 * u64::from(attempts);
                            tracing::warn!(
                                "Telegram post-connect error, retrying in {}ms (attempt {}/{}): {}",
                                backoff_ms,
                                attempts,
                                post_connect_max,
                                e
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        }
                        ErrorClass::Network(kind) => {
                            // Same retry budget as PreConnect (DNS / TLS /
                            // connect-timeout all imply "the server hasn't
                            // received our bytes"), but the log line names
                            // the actual network stage so the operator's first
                            // question ("is it DNS?") gets answered in the
                            // log instead of in a follow-up telemetry pull.
                            if attempts > max_retries {
                                cooldown.record_failure(conv_id, ErrorKind::Retryable);
                                if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                                    return Err(ChannelError::SendFailed(
                                        "Error suppressed by policy".to_string(),
                                    ));
                                }
                                return Err(ChannelError::SendFailed(e.to_string()));
                            }
                            let backoff_ms = 500 * u64::from(attempts);
                            tracing::warn!(
                                kind = ?kind,
                                "Telegram network error ({:?}), retrying in {}ms (attempt {}/{}): {}",
                                kind,
                                backoff_ms,
                                attempts,
                                max_retries,
                                e
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        }
                        ErrorClass::Forbidden(kind) => {
                            // Mirror `Rejected`'s shape: mark the conversation
                            // permanent (the right answer for chat-level
                            // forbiddens), surface the original error, bail.
                            // `MessageNotFound` is the one variant that maps
                            // to `Retryable` via `error_class_to_kind`, so a
                            // missing edit target on a single chunk does not
                            // park the whole conversation.
                            let kind_for_log = kind;
                            tracing::warn!(
                                kind = ?kind_for_log,
                                "Telegram forbidden ({:?}) — bailing without retry",
                                kind_for_log
                            );
                            cooldown.record_failure(
                                conv_id,
                                error_class_to_kind(&ErrorClass::Forbidden(kind_for_log)),
                            );
                            if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                                return Err(ChannelError::SendFailed(
                                    "Error suppressed by policy".to_string(),
                                ));
                            }
                            return Err(ChannelError::SendFailed(e.to_string()));
                        }
                    }
                }
            }
        };

        if is_first {
            first_msg = Some(sent);
        }
    }

    let sent = match first_msg {
        Some(msg) => msg,
        None => {
            let err =
                ChannelError::SendFailed("No message chunks to send (empty formatted text)".into());
            if !cooldown.should_send_error(conv_id, &config.error_policy, "") {
                return Err(ChannelError::SendFailed(
                    "Error suppressed by policy".to_string(),
                ));
            }
            return Err(err);
        }
    };

    // Send attachments if any (P2-A: photo albums — when ≥2 attachments are
    // image/*, send them as a single `sendMediaGroup` so Telegram renders
    // them as one album instead of N separate messages).
    if !message.attachments.is_empty() {
        send_attachments(bot, chat_id, thread_id, &message.attachments).await?;
    }

    // Delivery succeeded — clear any cooldown for this conversation
    cooldown.record_success(conv_id);

    Ok(SendResult {
        message_id: MessageId::new(sent.id.0.to_string()),
        timestamp: Utc::now(),
    })
}

// ---------------------------------------------------------------------------
// Typing indicator
// ---------------------------------------------------------------------------

/// Send a typing indicator to a conversation.
pub(crate) async fn send_typing(
    bot: &Bot,
    conversation_id: &str,
    _config: &ResolvedConfig,
    cooldown: &ErrorCooldown,
) -> ChannelResult<()> {
    // Skip if typing circuit breaker has tripped
    if !cooldown.check_typing() {
        tracing::debug!("typing indicator suppressed by circuit breaker");
        return Ok(());
    }

    let (chat_id, thread_id) = parse_conversation_id(conversation_id)?;

    let mut req = bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
    if let Some(tid) = thread_id {
        if tid != 1 {
            req = req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
        }
    }
    match req.await {
        Ok(_) => {
            cooldown.record_typing_success();
            Ok(())
        }
        Err(e) => {
            cooldown.record_typing_failure();
            Err(ChannelError::Internal(format!(
                "Failed to send typing: {e}"
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

/// Set or remove a reaction on a message.
pub(crate) async fn send_reaction(
    bot: &Bot,
    conversation_id: &str,
    message_id: &MessageId,
    reaction: &str,
) -> ChannelResult<()> {
    let (chat_id, _thread_id) = parse_conversation_id(conversation_id)?;

    let msg_id = teloxide::types::MessageId(
        message_id
            .as_str()
            .parse::<i32>()
            .map_err(|e| ChannelError::Internal(format!("Invalid message ID: {e}")))?,
    );

    let reactions = if reaction.is_empty() {
        vec![] // Remove reactions
    } else {
        vec![teloxide::types::ReactionType::Emoji {
            emoji: reaction.to_string(),
        }]
    };

    // Reactions are non-critical UX — swallow errors silently
    match bot
        .set_message_reaction(chat_id, msg_id)
        .reaction(reactions)
        .await
    {
        Ok(_) => {
            tracing::debug!(
                "Reaction '{}' set on message {}",
                reaction,
                message_id.as_str()
            );
            Ok(())
        }
        Err(e) => {
            tracing::debug!("Failed to set reaction (non-critical): {}", e);
            Ok(()) // Swallow — reactions are best-effort
        }
    }
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

/// Send a batch of attachments, picking `sendMediaGroup` when ≥2 of them
/// are images. Telegram requires every media-group entry to be the same
/// kind (photo+video mixes are rejected); the function below splits by kind
/// so each group is internally uniform.
///
/// Returns `Ok(())` on success and propagates the first error otherwise. A
/// partial send (some succeeded, some failed) is signalled by the upstream
/// cooldown — the caller treats any error as full failure and lets the
/// cooldown backoff schedule the retry.
///
/// `#[doc(hidden)]` — see `send_message` for the rationale. Exposed for
/// `tests/telegram_delivery_e2e.rs`.
#[doc(hidden)]
pub async fn send_attachments(
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<i32>,
    attachments: &[crate::gateway::channel::Attachment],
) -> ChannelResult<()> {
    if attachments.is_empty() {
        return Ok(());
    }

    // Pass 1: collect image attachments into groups of up to 10 (Telegram's
    // media-group ceiling). Non-image attachments and images whose MIME
    // could not be resolved fall through to the per-item path. The split
    // is by MIME up front so a `sendMediaGroup` never carries a video /
    // document — Telegram would 400 the whole batch.
    const ALBUM_MAX: usize = 10;
    let mut image_buf: Vec<&crate::gateway::channel::Attachment> = Vec::new();
    let mut singles: Vec<&crate::gateway::channel::Attachment> = Vec::new();

    for att in attachments {
        if att.mime_type.starts_with("image/") {
            image_buf.push(att);
        } else {
            // Flush any in-progress album so the per-item send can start
            // clean.
            if !image_buf.is_empty() {
                flush_image_album(bot, chat_id, thread_id, &image_buf).await?;
                image_buf.clear();
            }
            singles.push(att);
        }
        if image_buf.len() >= ALBUM_MAX {
            flush_image_album(bot, chat_id, thread_id, &image_buf).await?;
            image_buf.clear();
        }
    }
    if !image_buf.is_empty() {
        flush_image_album(bot, chat_id, thread_id, &image_buf).await?;
    }

    // Anything not in an album gets the per-item path (voice, video,
    // sticker, document, etc.).
    for att in singles {
        send_attachment(bot, chat_id, thread_id, att).await?;
    }
    Ok(())
}

/// Send one Telegram media-group with the buffered images.
///
/// Two-image threshold (not 1): a single image uses `sendPhoto`, which
/// renders with the bot's name as the byline and the photo's full metadata;
/// a media-group of one is functionally the same but loses the byline on
/// some clients (openclaw § 5.14 observation). We only batch when there
/// is at least one other image waiting, which is also when Telegram's UI
/// starts grouping them anyway.
async fn flush_image_album(
    bot: &Bot,
    chat_id: ChatId,
    thread_id: Option<i32>,
    images: &[&crate::gateway::channel::Attachment],
) -> ChannelResult<()> {
    use teloxide::types::{InputMedia, InputMediaPhoto};

    if images.len() < 2 {
        // Single image — never batch.
        for att in images {
            send_attachment(bot, chat_id, thread_id, att).await?;
        }
        return Ok(());
    }

    let media: Vec<InputMedia> = images
        .iter()
        .map(|att| {
            let input_file = attachment_to_input_file(att)?;
            Ok::<InputMedia, ChannelError>(InputMedia::Photo(InputMediaPhoto {
                media: input_file,
                caption: None,
                parse_mode: None,
                caption_entities: None,
                has_spoiler: false,
                show_caption_above_media: false,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut req = bot.send_media_group(chat_id, media);
    if let Some(tid) = thread_id {
        // Telegram rejects media-group with thread_id == 1 (General topic);
        // the per-item `send_attachment` path omits it for the same reason.
        if tid != 1 {
            req = req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
        }
    }
    req.await
        .map_err(|e| ChannelError::SendFailed(format!("Failed to send media group: {e}")))?;
    Ok(())
}

/// Convert a channel-agnostic `Attachment` to teloxide's `InputFile`.
/// Used by `flush_image_album` (and reserved for future album kinds) — the
/// per-MIME destination (send_photo vs send_voice) still lives in
/// `send_attachment` and the per-item fallback in `send_attachments`.
fn attachment_to_input_file(
    attachment: &crate::gateway::channel::Attachment,
) -> Result<teloxide::types::InputFile, ChannelError> {
    use teloxide::types::InputFile;
    if let Some(data) = &attachment.data {
        Ok(InputFile::memory(data.clone()))
    } else if let Some(path) = &attachment.path {
        Ok(InputFile::file(path))
    } else if let Some(url) = &attachment.url {
        url.parse()
            .map(InputFile::url)
            .map_err(|e| ChannelError::SendFailed(format!("Invalid attachment URL: {e}")))
    } else {
        Err(ChannelError::SendFailed(
            "Attachment has no data, path, or URL".to_string(),
        ))
    }
}

/// Send an attachment with optional forum-topic routing.
///
/// `#[doc(hidden)]` — see `send_message` for the rationale. Exposed for
/// `tests/telegram_delivery_e2e.rs`.
#[doc(hidden)]
pub async fn send_attachment(
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
        InputFile::url(
            url.parse()
                .map_err(|e| ChannelError::SendFailed(format!("Invalid attachment URL: {e}")))?,
        )
    } else {
        return Err(ChannelError::SendFailed(
            "Attachment has no data, path, or URL".to_string(),
        ));
    };

    // Determine attachment type by MIME type
    let mime = &attachment.mime_type;
    if mime == "image/webp" || mime == "application/x-tgsticker" || mime == "video/webm" {
        // Sticker formats: static (webp), animated (tgsticker), video (webm)
        let req = with_thread!(bot.send_sticker(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send sticker: {e}")))?;
    } else if mime.starts_with("image/") {
        let req = with_thread!(bot.send_photo(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send photo: {e}")))?;
    } else if mime == "audio/ogg" || mime == "audio/opus" || mime == "audio/ogg; codecs=opus" {
        // Voice messages: OGG/Opus → send as voice (inline playable)
        let req = with_thread!(bot.send_voice(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send voice: {e}")))?;
    } else if mime.starts_with("audio/") {
        // Other audio: MP3, WAV, etc. → also send as voice for TTS output
        let req = with_thread!(bot.send_voice(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send voice: {e}")))?;
    } else if mime.starts_with("video/") {
        // `Some(true)` triggers the round-bubble `sendVideoNote` dispatch;
        // `Some(false)` and `None` both fall through to the generic
        // `sendVideo` (the historical default). Other channels that ignore
        // the hint leave it `None` and pay no cost.
        if attachment.is_video_note {
            // Video note (round bubble) — Telegram-specific surface; ignored
            // by other channels via the `is_video_note` flag.
            let req = with_thread!(bot.send_video_note(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send video note: {e}")))?;
        } else {
            let req = with_thread!(bot.send_video(chat_id, input_file), thread_id);
            req.await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send video: {e}")))?;
        }
    } else if attachment.is_voice_note {
        // Voice-note hint on a non-audio MIME (e.g. MP4 audio track). Honour
        // the caller's intent — some TTS pipelines emit a video/* container
        // and want it rendered as the round bubble anyway.
        let req = with_thread!(bot.send_voice(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send voice: {e}")))?;
    } else {
        let req = with_thread!(bot.send_document(chat_id, input_file), thread_id);
        req.await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send document: {e}")))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Edit message
// ---------------------------------------------------------------------------

/// Edit a message's text and/or inline keyboard.
pub(crate) async fn edit_message(
    bot: &Bot,
    conversation_id: &str,
    message_id: &MessageId,
    new_text: Option<&str>,
    keyboard: Option<&InlineKeyboard>,
) -> ChannelResult<()> {
    let (chat, _thread_id) = parse_conversation_id(conversation_id)?;

    let msg_id = teloxide::types::MessageId(
        message_id
            .as_str()
            .parse()
            .map_err(|_| ChannelError::SendFailed("Invalid message ID".into()))?,
    );

    if let Some(text) = new_text {
        // Convert Markdown to Telegram HTML for consistent rendering
        let html_text = MessageFormatter::format(text, MarkupFormat::TelegramHtml);

        // Edit text (and optionally keyboard)
        let mut request = bot
            .edit_message_text(chat, msg_id, &html_text)
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

        match request.await {
            Ok(_) => {}
            Err(ref e) if is_benign_edit_error(e) => {
                tracing::debug!("edit_message: benign error ignored: {}", e);
            }
            Err(e) => {
                return Err(ChannelError::SendFailed(e.to_string()));
            }
        }
    } else if let Some(kb) = keyboard {
        // Edit only the keyboard (need to use edit_message_reply_markup)
        let markup = InlineKeyboardMarkup::new(
            kb.rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|btn| InlineKeyboardButton::callback(&btn.text, &btn.callback_data))
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

// ---------------------------------------------------------------------------
// Orchestrator delivery wrapper
// ---------------------------------------------------------------------------

use crate::sync_primitives::Arc;

/// Cloneable wrapper around Telegram delivery primitives for the streaming orchestrator.
#[derive(Clone)]
pub struct TelegramDelivery {
    pub bot: Bot,
    pub config: ResolvedConfig,
    pub(crate) cooldown: Arc<ErrorCooldown>,
    pub conversation_id: String,
}

impl TelegramDelivery {
    pub(crate) fn new(
        bot: Bot,
        config: ResolvedConfig,
        cooldown: Arc<ErrorCooldown>,
        conversation_id: impl Into<String>,
    ) -> Self {
        Self {
            bot,
            config,
            cooldown,
            conversation_id: conversation_id.into(),
        }
    }

    /// Send a plain text message and return its Telegram message ID.
    pub async fn send_text_message(&self, text: &str) -> ChannelResult<i64> {
        let (chat_id, thread_id) = parse_conversation_id(&self.conversation_id)?;
        let html_text = MessageFormatter::format(text, MarkupFormat::TelegramHtml);
        let mut req = with_thread!(
            self.bot
                .send_message(chat_id, &html_text)
                .parse_mode(ParseMode::Html),
            thread_id
        );
        if let Some(opts) = link_preview_options(self.config.link_preview) {
            req = req.link_preview_options(opts);
        }
        match req.await {
            Ok(msg) => Ok(i64::from(msg.id.0)),
            Err(e) => {
                let cls = classify_error(&e);
                self.cooldown
                    .record_failure(&self.conversation_id, error_class_to_kind(&cls));
                Err(ChannelError::SendFailed(e.to_string()))
            }
        }
    }

    /// Edit an existing message.
    pub async fn edit_text_message(&self, message_id: i64, text: &str) -> ChannelResult<()> {
        let (chat_id, _thread_id) = parse_conversation_id(&self.conversation_id)?;
        let msg_id = teloxide::types::MessageId(message_id as i32);
        let html_text = MessageFormatter::format(text, MarkupFormat::TelegramHtml);
        let mut request = self
            .bot
            .edit_message_text(chat_id, msg_id, &html_text)
            .parse_mode(ParseMode::Html);
        if let Some(opts) = link_preview_options(self.config.link_preview) {
            request = request.link_preview_options(opts);
        }
        match request.await {
            Ok(_) => Ok(()),
            Err(ref e) if is_benign_edit_error(e) => {
                tracing::debug!("edit_text_message: benign error ignored: {}", e);
                Ok(())
            }
            Err(e) => Err(ChannelError::SendFailed(e.to_string())),
        }
    }

    /// Set a reaction on a message.
    pub async fn set_reaction(&self, message_id: i64, emoji: &str) -> ChannelResult<()> {
        let (chat_id, _thread_id) = parse_conversation_id(&self.conversation_id)?;
        let msg_id = teloxide::types::MessageId(message_id as i32);
        let reactions = if emoji.is_empty() {
            vec![]
        } else {
            vec![teloxide::types::ReactionType::Emoji {
                emoji: emoji.to_string(),
            }]
        };
        match self
            .bot
            .set_message_reaction(chat_id, msg_id)
            .reaction(reactions)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::debug!("Failed to set reaction (non-critical): {}", e);
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_preview_enabled_leaves_request_untouched() {
        // `Enabled` is Telegram's default — no options emitted.
        assert!(link_preview_options(LinkPreviewMode::Enabled).is_none());
    }

    #[test]
    fn test_link_preview_disabled_suppresses_card() {
        let opts = link_preview_options(LinkPreviewMode::Disabled).expect("options");
        assert!(opts.is_disabled);
        assert!(!opts.show_above_text);
    }

    #[test]
    fn test_link_preview_above_keeps_card_above_text() {
        let opts = link_preview_options(LinkPreviewMode::Above).expect("options");
        assert!(!opts.is_disabled);
        assert!(opts.show_above_text);
    }

    #[test]
    fn test_parse_conversation_id_plain() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789").unwrap();
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, None);
    }

    #[test]
    fn test_parse_conversation_id_with_topic() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789:topic:42").unwrap();
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(42));
    }

    #[test]
    fn test_parse_conversation_id_general_topic() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789:topic:1").unwrap();
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(1));
    }

    #[test]
    fn test_parse_conversation_id_invalid_chat() {
        let result = parse_conversation_id("not_a_number");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid conversation_id"));
    }

    #[test]
    fn test_parse_conversation_id_invalid_thread() {
        let result = parse_conversation_id("-100123456789:topic:not_a_number");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid thread_id"));
    }

    // -----------------------------------------------------------------------
    // Error classification tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_classify_rate_limited() {
        use teloxide::types::Seconds;
        let seconds = Seconds::from_seconds(30);
        let err = teloxide::RequestError::RetryAfter(seconds);
        match classify_error(&err) {
            ErrorClass::RateLimited(secs) => assert_eq!(secs, 30),
            other => panic!("Expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_bot_blocked() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::BotBlocked);
        match classify_error(&err) {
            ErrorClass::Forbidden(ForbiddenKind::BotBlocked) => {}
            other => panic!("Expected Forbidden(BotBlocked), got {:?}", other),
        }
    }

    #[test]
    fn test_classify_chat_not_found() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::ChatNotFound);
        match classify_error(&err) {
            ErrorClass::Forbidden(ForbiddenKind::ChatNotFound) => {}
            other => panic!("Expected Forbidden(ChatNotFound), got {:?}", other),
        }
    }

    #[test]
    fn test_classify_user_not_found() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::UserNotFound);
        match classify_error(&err) {
            ErrorClass::Forbidden(ForbiddenKind::UserNotFound) => {}
            other => panic!("Expected Forbidden(UserNotFound), got {:?}", other),
        }
    }

    #[test]
    fn test_classify_invalid_token() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::InvalidToken);
        match classify_error(&err) {
            ErrorClass::Rejected(_) => {}
            other => panic!("Expected Rejected, got {:?}", other),
        }
    }
}

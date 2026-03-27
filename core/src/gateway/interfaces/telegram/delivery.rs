//! Telegram message delivery — send, retry, chunking, attachments.
//!
//! All outbound message operations extracted from the Channel trait impl.
//! Each function takes a `&Bot` directly (no `&self`), making them
//! independently testable and reusable from both Channel and MessageOps.

use crate::gateway::channel::{
    Attachment, ChannelError, ChannelResult, InlineKeyboard, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use teloxide::{
    prelude::*,
    types::{
        ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, ParseMode, ThreadId,
    },
};

use super::config::TelegramConfig;

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Classification of Telegram API errors for retry logic.
#[derive(Debug)]
pub(crate) enum ErrorClass {
    /// DNS/TCP failure — safe to retry, data never sent.
    PreConnect,
    /// Timeout/reset — may have been sent, retry cautiously.
    PostConnect,
    /// Telegram API rejection — don't retry, fallback to plain text.
    Rejected(String),
    /// 429 rate limit — wait exact seconds then retry.
    RateLimited(u64),
}

/// Classify a teloxide request error for retry decisions.
///
/// Uses teloxide's typed enums (`ApiError`, `RequestError::RetryAfter`) and
/// reqwest's `is_connect()` for precise classification instead of fragile
/// string matching.
pub(crate) fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        // Rate limit is a top-level RequestError variant (not inside ApiError)
        teloxide::RequestError::RetryAfter(seconds) => {
            ErrorClass::RateLimited(seconds.seconds() as u64)
        }
        teloxide::RequestError::Api(api_err) => {
            use teloxide::ApiError;
            match api_err {
                // Permanent rejections — user blocked bot, chat/user gone
                ApiError::BotBlocked | ApiError::ChatNotFound | ApiError::UserNotFound => {
                    ErrorClass::Rejected(api_err.to_string())
                }
                // Invalid token is also permanent
                ApiError::InvalidToken => ErrorClass::Rejected(api_err.to_string()),
                // Catch other permanent errors by message content
                _ => {
                    let msg = api_err.to_string();
                    if msg.contains("Bad Request") {
                        ErrorClass::Rejected(msg)
                    } else {
                        ErrorClass::PostConnect
                    }
                }
            }
        }
        teloxide::RequestError::Network(reqwest_err) => {
            if reqwest_err.is_connect() {
                ErrorClass::PreConnect // DNS/TCP failure — data never sent
            } else {
                ErrorClass::PostConnect // timeout, reset, etc.
            }
        }
        _ => ErrorClass::PostConnect,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a conversation_id that may contain a forum topic suffix.
///
/// Format: `"{chat_id}"` or `"{chat_id}:topic:{thread_id}"`.
/// Returns the `ChatId` and an optional raw thread id (i32).
pub(crate) fn parse_conversation_id(conv_id: &str) -> (ChatId, Option<i32>) {
    if let Some((chat, topic)) = conv_id.split_once(":topic:") {
        (
            ChatId(chat.parse().unwrap_or(0)),
            topic.parse().ok(),
        )
    } else {
        (ChatId(conv_id.parse().unwrap_or(0)), None)
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
// HTML-safe chunking
// ---------------------------------------------------------------------------

/// Telegram-supported HTML tags.
const TELEGRAM_TAGS: &[&str] = &["b", "i", "s", "u", "code", "pre", "blockquote", "tg-spoiler"];

/// Reserve space for closing/reopening tags at chunk boundaries.
const TAG_OVERHEAD: usize = 200;

/// Analyse a chunk of HTML and return the unclosed tags.
///
/// Returns `(tags_to_close, tags_to_reopen)` where:
/// - `tags_to_close` — tag names in reverse nesting order (innermost first)
/// - `tags_to_reopen` — same tag names in original nesting order (outermost first)
pub(crate) fn balance_html_tags(chunk: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut stack: Vec<&'static str> = Vec::new();
    let bytes = chunk.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'<' {
            // Find the end of this tag
            if let Some(close_pos) = chunk[i..].find('>') {
                let tag_content = &chunk[i + 1..i + close_pos];
                let is_closing = tag_content.starts_with('/');
                let tag_name_raw = if is_closing {
                    &tag_content[1..]
                } else {
                    // Strip attributes if any
                    tag_content.split_whitespace().next().unwrap_or("")
                };
                let tag_name_lower = tag_name_raw.to_lowercase();

                // Match against known Telegram tags
                if let Some(&known) = TELEGRAM_TAGS.iter().find(|&&t| t == tag_name_lower) {
                    if is_closing {
                        // Pop from stack if matching
                        if let Some(pos) = stack.iter().rposition(|&t| t == known) {
                            stack.remove(pos);
                        }
                    } else {
                        stack.push(known);
                    }
                }

                i += close_pos + 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // tags_to_close: reverse order (innermost first)
    let mut close = stack.clone();
    close.reverse();
    // tags_to_reopen: original nesting order (outermost first)
    let reopen = stack;
    (close, reopen)
}

/// Split HTML text into chunks that respect tag integrity.
///
/// Each chunk will have balanced HTML tags — unclosed tags at the end of a
/// chunk are closed, and reopened at the start of the next chunk.
/// `max_len` is in Rust chars (not bytes).
pub(crate) fn split_html_safe(html: &str, max_len: usize) -> Vec<String> {
    if html.chars().count() <= max_len {
        return vec![html.to_string()];
    }

    let effective_limit = if max_len > TAG_OVERHEAD {
        max_len - TAG_OVERHEAD
    } else {
        max_len
    };

    let mut chunks: Vec<String> = Vec::new();
    let remaining = html;

    while !remaining.is_empty() {
        let char_count = remaining.chars().count();
        if char_count <= max_len {
            // Last chunk fits entirely
            chunks.push(remaining.to_string());
            break;
        }

        // Find byte offset of the effective_limit-th char
        let byte_limit = remaining
            .char_indices()
            .nth(effective_limit)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());

        let search_slice = &remaining[..byte_limit];

        // Split priority: \n\n > \n > space > hard cut
        let split_byte_pos = search_slice
            .rfind("\n\n")
            .map(|p| p + 2) // include the double newline in current chunk
            .or_else(|| search_slice.rfind('\n').map(|p| p + 1))
            .or_else(|| search_slice.rfind(' ').map(|p| p + 1))
            .unwrap_or(byte_limit);

        // Safety: ensure we don't split at 0 (infinite loop guard)
        let split_byte_pos = if split_byte_pos == 0 {
            byte_limit
        } else {
            split_byte_pos
        };

        let chunk_text = &remaining[..split_byte_pos];
        let rest = &remaining[split_byte_pos..];

        // Balance HTML tags
        let (close_tags, reopen_tags) = balance_html_tags(chunk_text);

        let mut chunk_str = chunk_text.to_string();
        // Append closing tags to current chunk
        for tag in &close_tags {
            chunk_str.push_str(&format!("</{}>", tag));
        }

        chunks.push(chunk_str);

        // Prepend reopening tags to next chunk
        if !rest.is_empty() {
            let mut prefix = String::new();
            for tag in &reopen_tags {
                prefix.push_str(&format!("<{}>", tag));
            }
            // We need to own the rest with prefix prepended
            let new_remaining = format!("{}{}", prefix, rest);
            // Since remaining is a slice of the original, we need to handle ownership
            // We'll collect the rest into an owned string and continue with a different approach
            // Recurse on the rest with reopened tags prepended
            let sub_chunks = split_html_safe(&new_remaining, max_len);
            chunks.extend(sub_chunks);
            break;
        } else {
            break;
        }
    }

    chunks
}

// ---------------------------------------------------------------------------
// Core send
// ---------------------------------------------------------------------------

/// Send an outbound message with chunking, retry, and attachment support.
///
/// This is the extracted body of `Channel::send()`.
pub(crate) async fn send_message(
    bot: &Bot,
    config: &TelegramConfig,
    message: &OutboundMessage,
) -> ChannelResult<SendResult> {
    let (chat_id, thread_id) = parse_conversation_id(message.conversation_id.as_str());

    // Send typing indicator if enabled
    if config.send_typing {
        let mut action_req =
            bot.send_chat_action(chat_id, teloxide::types::ChatAction::Typing);
        if let Some(tid) = thread_id {
            if tid != 1 {
                action_req =
                    action_req.message_thread_id(ThreadId(teloxide::types::MessageId(tid)));
            }
        }
        let _ = action_req.await;
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
            let result = build_request(Some(ParseMode::Html), chunk, reply_to_ref, keyboard_ref).await;
            match result {
                Ok(msg) => break msg,
                Err(e) => {
                    attempts += 1;
                    match classify_error(&e) {
                        ErrorClass::Rejected(reason) => {
                            // Permanent rejection — try plain text fallback (strip HTML)
                            tracing::warn!(
                                "HTML send rejected ({}), falling back to plain text",
                                reason
                            );
                            break build_request(None, chunk, reply_to_ref, keyboard_ref)
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
                            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                        }
                        ErrorClass::PreConnect => {
                            // DNS/TCP failure — data never sent, safe to retry aggressively
                            if attempts > max_retries {
                                return Err(ChannelError::SendFailed(e.to_string()));
                            }
                            let backoff_ms = 500 * attempts as u64;
                            tracing::warn!(
                                "Telegram pre-connect error, retrying in {}ms (attempt {}/{}): {}",
                                backoff_ms, attempts, max_retries, e
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        }
                        ErrorClass::PostConnect => {
                            // Data may have been sent — limit retries to avoid duplicates
                            let post_connect_max = max_retries.min(2);
                            if attempts > post_connect_max {
                                return Err(ChannelError::SendFailed(e.to_string()));
                            }
                            let backoff_ms = 1000 * attempts as u64;
                            tracing::warn!(
                                "Telegram post-connect error, retrying in {}ms (attempt {}/{}): {}",
                                backoff_ms, attempts, post_connect_max, e
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
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
        send_attachment(bot, chat_id, thread_id, attachment).await?;
    }

    Ok(SendResult {
        message_id: MessageId::new(sent.id.0.to_string()),
        timestamp: Utc::now(),
    })
}

// ---------------------------------------------------------------------------
// Typing indicator
// ---------------------------------------------------------------------------

/// Send a typing indicator to a conversation.
pub(crate) async fn send_typing(bot: &Bot, conversation_id: &str) -> ChannelResult<()> {
    let (chat_id, thread_id) = parse_conversation_id(conversation_id);

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
    let (chat_id, _thread_id) = parse_conversation_id(conversation_id);

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

/// Send an attachment with optional forum-topic routing.
pub(crate) async fn send_attachment(
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
    let (chat, _thread_id) = parse_conversation_id(conversation_id);

    let msg_id = teloxide::types::MessageId(message_id.as_str().parse().map_err(|_| {
        ChannelError::SendFailed("Invalid message ID".into())
    })?);

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_conversation_id_plain() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, None);
    }

    #[test]
    fn test_parse_conversation_id_with_topic() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789:topic:42");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(42));
    }

    #[test]
    fn test_parse_conversation_id_general_topic() {
        let (chat_id, thread_id) = parse_conversation_id("-100123456789:topic:1");
        assert_eq!(chat_id.0, -100123456789);
        assert_eq!(thread_id, Some(1));
    }

    #[test]
    fn test_parse_conversation_id_invalid() {
        let (chat_id, thread_id) = parse_conversation_id("not_a_number");
        assert_eq!(chat_id.0, 0);
        assert_eq!(thread_id, None);
    }

    // -----------------------------------------------------------------------
    // HTML-safe chunking tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_balance_html_tags_no_tags() {
        let (close, open) = balance_html_tags("hello world");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }

    #[test]
    fn test_balance_html_tags_unclosed() {
        let (close, open) = balance_html_tags("<b>hello");
        assert_eq!(close, vec!["b"]);
        assert_eq!(open, vec!["b"]);
    }

    #[test]
    fn test_balance_html_tags_closed() {
        let (close, open) = balance_html_tags("<b>hello</b>");
        assert!(close.is_empty());
        assert!(open.is_empty());
    }

    #[test]
    fn test_split_html_safe_short_text() {
        let chunks = split_html_safe("<b>hello</b>", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "<b>hello</b>");
    }

    #[test]
    fn test_split_html_safe_balances_bold() {
        let inner = "x".repeat(100);
        let html = format!("<b>{}</b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
        assert!(chunks[0].ends_with("</b>"), "first chunk should end with </b>: {}", chunks[0]);
        assert!(chunks[1].starts_with("<b>"), "second chunk should start with <b>: {}", chunks[1]);
    }

    #[test]
    fn test_split_html_safe_prefers_newline() {
        let html = format!("{}\n{}", "a".repeat(50), "b".repeat(50));
        let chunks = split_html_safe(&html, 60);
        assert_eq!(chunks.len(), 2, "expected 2 chunks, got {}: {:?}", chunks.len(), chunks);
        assert_eq!(chunks[0].trim(), &"a".repeat(50));
    }

    #[test]
    fn test_split_html_safe_nested_tags() {
        let inner = "x".repeat(100);
        let html = format!("<b><i>{}</i></b>", inner);
        let chunks = split_html_safe(&html, 60);
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
        assert!(chunks[0].ends_with("</i></b>"), "first chunk should end with </i></b>: {}", chunks[0]);
        assert!(chunks[1].starts_with("<b><i>"), "second chunk should start with <b><i>: {}", chunks[1]);
    }

    #[test]
    fn test_split_html_safe_utf8_safety() {
        let text = "你好世界".repeat(30); // 120 CJK chars
        let chunks = split_html_safe(&text, 50);
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
        for chunk in &chunks {
            // max_len=50 but last chunk or overhead may cause slight overshoot
            // with TAG_OVERHEAD the effective limit is small, so chunks should be well under max_len
            assert!(
                chunk.chars().count() <= 55,
                "chunk too long: {} chars",
                chunk.chars().count()
            );
        }
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
            ErrorClass::Rejected(_) => {}
            other => panic!("Expected Rejected, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_chat_not_found() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::ChatNotFound);
        match classify_error(&err) {
            ErrorClass::Rejected(_) => {}
            other => panic!("Expected Rejected, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_user_not_found() {
        let err = teloxide::RequestError::Api(teloxide::ApiError::UserNotFound);
        match classify_error(&err) {
            ErrorClass::Rejected(_) => {}
            other => panic!("Expected Rejected, got {:?}", other),
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

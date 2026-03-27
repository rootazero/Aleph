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
    /// Transient error (network, server-side) — safe to retry.
    Recoverable,
    /// Permanent error (bad request, unauthorized) — do not retry.
    Unrecoverable,
    /// Rate limited by Telegram — wait the given seconds before retrying.
    RateLimited(u64),
}

/// Classify a teloxide request error for retry decisions.
pub(crate) fn classify_error(err: &teloxide::RequestError) -> ErrorClass {
    match err {
        teloxide::RequestError::Api(api_err) => {
            let msg = api_err.to_string();
            if msg.contains("Too Many Requests") || msg.contains("429") {
                ErrorClass::RateLimited(30)
            } else if msg.contains("Unauthorized")
                || msg.contains("401")
                || msg.contains("Bad Request")
                || msg.contains("400")
            {
                ErrorClass::Unrecoverable
            } else {
                ErrorClass::Recoverable
            }
        }
        teloxide::RequestError::Network(_) => ErrorClass::Recoverable,
        _ => ErrorClass::Recoverable,
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
        let html_text = MessageFormatter::format(chunk, MarkupFormat::TelegramHtml);
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
            let result = build_request(Some(ParseMode::Html), &html_text, reply_to_ref, keyboard_ref).await;
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
                        ErrorClass::Recoverable => {
                            if attempts > max_retries {
                                return Err(ChannelError::SendFailed(e.to_string()));
                            }
                            let backoff_ms = 500 * attempts as u64;
                            tracing::warn!(
                                "Telegram send error (recoverable), retrying in {}ms (attempt {}/{}): {}",
                                backoff_ms, attempts, max_retries, e
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
}

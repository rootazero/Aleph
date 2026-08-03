//! Message conversion and attachment extraction for the Telegram channel.
//!
//! These are pure functions that convert teloxide types into the channel-agnostic
//! `InboundMessage` / `Attachment` types. Extracted from `mod.rs` to keep the
//! top-level module focused on channel lifecycle and trait implementation.
//!
//! # Telegram-InboundContext Integration
//!
//! `TelegramInboundContext` is constructed in `mod.rs` (where we have access to
//! the original `teloxide::types::Message`) using `TelegramInboundContext::from_inbound()`.
//! Key Telegram-specific metadata (`thread_id`, `sender_id`) is also passed via
//! `InboundMessage.metadata` so the router can access it if needed.

use super::sticker::StickerPipeline;
use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, MessageMeta, UserId,
};
use chrono::{TimeZone, Utc};
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{MediaKind, MessageKind, MessageOrigin};

/// Convert a Telegram message to an [`InboundMessage`].
///
/// Access control is handled by the caller via [`AccessController::check_message`].
/// This function only performs message content extraction; it assumes the caller
/// has already verified that the message is allowed.
pub(crate) async fn convert_message(
    msg: &teloxide::types::Message,
    bot: &Bot,
    channel_id: &ChannelId,
    sticker_pipeline: &StickerPipeline,
) -> Option<InboundMessage> {
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

    let is_group = msg.chat.is_group() || msg.chat.is_supergroup();

    // Extract attachments first (async — resolves file URLs via Bot API)
    let attachments = extract_attachments(msg, bot).await;

    // Extract text content
    let text = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(text_msg) => text_msg.text.clone(),
            MediaKind::Photo(photo) => photo.caption.clone().unwrap_or_default(),
            MediaKind::Document(doc) => doc.caption.clone().unwrap_or_default(),
            MediaKind::Audio(audio) => audio.caption.clone().unwrap_or_default(),
            MediaKind::Video(video) => video.caption.clone().unwrap_or_default(),
            MediaKind::Voice(voice) => voice.caption.clone().unwrap_or_default(),
            MediaKind::Sticker(s) => {
                let emoji = s.sticker.emoji.as_deref().unwrap_or("?");
                let file_unique_id = &s.sticker.file.unique_id.0;
                let desc = sticker_pipeline
                    .resolve_description(file_unique_id, "")
                    .await;
                match desc {
                    Some(d) => format!("[Sticker: {emoji} | {d}]"),
                    None => format!("[Sticker: {emoji}]"),
                }
            }
            _ => String::new(),
        },
        _ => return None, // Ignore non-common messages (service messages, etc.)
    };

    // Skip messages with no text AND no attachments
    if text.is_empty() && attachments.is_empty() {
        return None;
    }

    // Get reply-to message ID
    let reply_to = msg
        .reply_to_message()
        .map(|r| MessageId::new(r.id.0.to_string()));

    // Convert timestamp
    let timestamp = Utc
        .timestamp_opt(msg.date.timestamp(), 0)
        .single()
        .unwrap_or_else(Utc::now);

    // Encode forum topic into conversation_id for session isolation.
    // Format: "{chat_id}" or "{chat_id}:topic:{thread_id}"
    let conversation_id = if let Some(thread_id) = msg.thread_id {
        ConversationId::new(format!("{}:topic:{}", msg.chat.id.0, thread_id.0 .0))
    } else {
        ConversationId::new(msg.chat.id.0.to_string())
    };

    // Extract platform-specific metadata
    let mut metadata: Vec<MessageMeta> = Vec::new();

    // Bot-authored: surfaced for the inbound router's pair-loop-guard so it can
    // track bot↔bot traffic. Filtering happens in the router, not here.
    if msg.from.as_ref().is_some_and(|u| u.is_bot) {
        metadata.push(MessageMeta::BotAuthored);
    }

    // media_group_id — messages sharing this ID belong to one album
    if let Some(group_id) = msg.media_group_id() {
        metadata.push(MessageMeta::MediaGroupId(group_id.to_string()));
    }

    // forward_origin — extract sender name from whichever variant is present
    if let MessageKind::Common(ref common) = msg.kind {
        if let Some(ref origin) = common.forward_origin {
            let (sender_name, date) = match origin {
                MessageOrigin::User { date, sender_user } => {
                    let name = sender_user
                        .username
                        .clone()
                        .unwrap_or_else(|| sender_user.first_name.clone());
                    (Some(name), Some(*date))
                }
                MessageOrigin::HiddenUser {
                    date,
                    sender_user_name,
                } => (Some(sender_user_name.clone()), Some(*date)),
                MessageOrigin::Chat {
                    date, sender_chat, ..
                } => {
                    let name = sender_chat.title().map(|t| t.to_string());
                    (name, Some(*date))
                }
                MessageOrigin::Channel { date, chat, .. } => {
                    let name = chat.title().map(|t| t.to_string());
                    (name, Some(*date))
                }
            };
            metadata.push(MessageMeta::ForwardOrigin { sender_name, date });
        }
    }

    if !attachments.is_empty() {
        let mime_types: Vec<&str> = attachments.iter().map(|a| a.mime_type.as_str()).collect();
        tracing::info!(
            target: "multimodal",
            probe = "P1_inbound",
            channel = "telegram",
            chat_id = %msg.chat.id.0,
            message_id = %msg.id.0,
            attachment_count = attachments.len(),
            mime_types = %mime_types.join(","),
            "Inbound message with attachments"
        );
    }

    Some(InboundMessage {
        id: MessageId::new(msg.id.0.to_string()),
        channel_id: channel_id.clone(),
        conversation_id,
        sender_id,
        sender_name,
        text,
        attachments,
        timestamp,
        reply_to,
        is_group,
        raw: Some(serde_json::to_value(msg).unwrap_or_default()),
        metadata,
    })
}

/// Extract attachments from a Telegram message, resolving file URLs via Bot API.
pub(crate) async fn extract_attachments(
    msg: &teloxide::types::Message,
    bot: &Bot,
) -> Vec<Attachment> {
    // Collect (file_id, mime_type, filename, size) from each media kind
    let media_info: Option<(String, String, Option<String>, u64)> =
        if let MessageKind::Common(common) = &msg.kind {
            match &common.media_kind {
                MediaKind::Photo(photo) => photo.photo.last().map(|largest| {
                    (
                        largest.file.id.0.clone(),
                        "image/jpeg".to_string(),
                        None,
                        u64::from(largest.file.size),
                    )
                }),
                MediaKind::Document(doc) => Some((
                    doc.document.file.id.0.clone(),
                    doc.document
                        .mime_type
                        .as_ref()
                        .map_or_else(|| "application/octet-stream".to_string(), |m| m.to_string()),
                    doc.document.file_name.clone(),
                    u64::from(doc.document.file.size),
                )),
                MediaKind::Audio(audio) => Some((
                    audio.audio.file.id.0.clone(),
                    audio
                        .audio
                        .mime_type
                        .as_ref()
                        .map_or_else(|| "audio/mpeg".to_string(), |m| m.to_string()),
                    audio.audio.file_name.clone(),
                    u64::from(audio.audio.file.size),
                )),
                MediaKind::Video(video) => Some((
                    video.video.file.id.0.clone(),
                    video
                        .video
                        .mime_type
                        .as_ref()
                        .map_or_else(|| "video/mp4".to_string(), |m| m.to_string()),
                    video.video.file_name.clone(),
                    u64::from(video.video.file.size),
                )),
                MediaKind::Voice(voice) => Some((
                    voice.voice.file.id.0.clone(),
                    voice
                        .voice
                        .mime_type
                        .as_ref()
                        .map_or_else(|| "audio/ogg".to_string(), |m| m.to_string()),
                    None,
                    u64::from(voice.voice.file.size),
                )),
                MediaKind::Sticker(s) => {
                    let mime = if s.sticker.flags.is_animated {
                        "application/x-tgsticker".to_string()
                    } else if s.sticker.flags.is_video {
                        "video/webm".to_string()
                    } else {
                        "image/webp".to_string()
                    };
                    Some((
                        s.sticker.file.id.0.clone(),
                        mime,
                        None,
                        u64::from(s.sticker.file.size),
                    ))
                }
                _ => None,
            }
        } else {
            None
        };

    let Some((file_id, mime_type, filename, size)) = media_info else {
        return Vec::new();
    };

    // Resolve file URL via Bot API (with 5s timeout to prevent handler blocking)
    let url = match tokio::time::timeout(
        Duration::from_secs(5),
        bot.get_file(teloxide::types::FileId(file_id.clone())),
    )
    .await
    {
        Ok(Ok(file)) => Some(format!(
            "https://api.telegram.org/file/bot{}/{}",
            bot.token(),
            file.path
        )),
        Ok(Err(e)) => {
            tracing::warn!("Failed to resolve file URL for {}: {}", file_id, e);
            None
        }
        Err(_elapsed) => {
            tracing::warn!("Timed out resolving file URL for {} (5s)", file_id);
            None
        }
    };

    vec![Attachment {
        id: file_id,
        mime_type,
        filename,
        size: Some(size),
        url,
        path: None,
        data: None,
    }]
}

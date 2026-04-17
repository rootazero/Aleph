use crate::gateway::channel::{
    ChannelError, ChannelResult, ConversationId, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use crate::gateway::interfaces::matrix::media;
use chrono::Utc;
use matrix_sdk::ruma::{
    events::{
        reaction::ReactionEventContent,
        relation::Annotation,
        room::message::{
            AudioMessageEventContent, FileMessageEventContent, ImageMessageEventContent, Relation,
            ReplacementMetadata, RoomMessageEventContent, VideoMessageEventContent,
        },
    },
    OwnedEventId, OwnedRoomId,
};
use matrix_sdk::Client;

pub const MATRIX_MSG_LIMIT: usize = 65535;

pub async fn send_message(client: &Client, message: OutboundMessage) -> ChannelResult<SendResult> {
    let room_id: OwnedRoomId = message
        .conversation_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid room ID: {e}")))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| ChannelError::SendFailed(format!("Room {} not found", room_id)))?;

    let reply_to = message.reply_to.as_ref().map(|id| id.as_str());

    if !message.attachments.is_empty() {
        return send_with_attachments(client, &room, message).await;
    }

    let formatted = MessageFormatter::format(&message.text, MarkupFormat::Markdown);
    let chunks = MessageFormatter::split(&formatted, MATRIX_MSG_LIMIT);

    let mut last_result = None;
    for (idx, chunk) in chunks.iter().enumerate() {
        let mut content = RoomMessageEventContent::text_html(chunk, chunk);

        if idx == 0 {
            if let Some(event_id_str) = reply_to {
                let event_id: OwnedEventId = event_id_str.parse().map_err(|e| {
                    ChannelError::SendFailed(format!("Invalid reply event ID: {e}"))
                })?;
                content.relates_to = Some(Relation::Reply {
                    in_reply_to: matrix_sdk::ruma::events::relation::InReplyTo::new(event_id),
                });
            }
        }

        let response = room
            .send(content)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send message: {e}")))?;

        last_result = Some(SendResult {
            message_id: MessageId::new(response.event_id.to_string()),
            timestamp: Utc::now(),
        });
    }

    last_result.ok_or_else(|| ChannelError::SendFailed("No message content to send".to_string()))
}

async fn send_with_attachments(
    client: &Client,
    room: &matrix_sdk::room::Room,
    message: OutboundMessage,
) -> ChannelResult<SendResult> {
    let reply_to = message.reply_to.as_ref().map(|id| id.as_str().to_string());
    let mut last_result = None;

    if !message.text.is_empty() {
        let formatted = MessageFormatter::format(&message.text, MarkupFormat::Markdown);
        let chunks = MessageFormatter::split(&formatted, MATRIX_MSG_LIMIT);

        for (idx, chunk) in chunks.iter().enumerate() {
            let mut content = RoomMessageEventContent::text_html(chunk, chunk);

            if idx == 0 {
                if let Some(ref event_id_str) = reply_to {
                    let event_id: OwnedEventId = event_id_str.parse().map_err(|e| {
                        ChannelError::SendFailed(format!("Invalid reply event ID: {e}"))
                    })?;
                    content.relates_to = Some(Relation::Reply {
                        in_reply_to: matrix_sdk::ruma::events::relation::InReplyTo::new(event_id),
                    });
                }
            }

            let response = room
                .send(content)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Failed to send message: {e}")))?;

            last_result = Some(SendResult {
                message_id: MessageId::new(response.event_id.to_string()),
                timestamp: Utc::now(),
            });
        }
    }

    for attachment in &message.attachments {
        let (content_bytes, mime_type, filename) = prepare_attachment(attachment).await?;
        let mxc_uri =
            media::upload_media(client, content_bytes, &mime_type, filename.as_deref()).await?;

        let body = filename.clone().unwrap_or_else(|| "attachment".to_string());
        let event_content = build_media_message(&mime_type, &body, &mxc_uri)?;

        let response = room
            .send(event_content)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to send attachment: {e}")))?;

        last_result = Some(SendResult {
            message_id: MessageId::new(response.event_id.to_string()),
            timestamp: Utc::now(),
        });
    }

    last_result
        .ok_or_else(|| ChannelError::SendFailed("No message or attachments to send".to_string()))
}

fn build_media_message(
    mime_type: &str,
    body: &str,
    mxc_uri: &str,
) -> ChannelResult<matrix_sdk::ruma::events::AnyMessageLikeEventContent> {
    use matrix_sdk::ruma::events::room::message::MessageType;
    use matrix_sdk::ruma::OwnedMxcUri;

    let uri = OwnedMxcUri::from(mxc_uri);
    if !uri.is_valid() {
        return Err(ChannelError::SendFailed(format!(
            "Invalid mxc URI: {}",
            mxc_uri
        )));
    }

    let msg_type = if mime_type.starts_with("image/") {
        MessageType::Image(ImageMessageEventContent::plain(body.to_string(), uri))
    } else if mime_type.starts_with("audio/") {
        MessageType::Audio(AudioMessageEventContent::plain(body.to_string(), uri))
    } else if mime_type.starts_with("video/") {
        MessageType::Video(VideoMessageEventContent::plain(body.to_string(), uri))
    } else {
        MessageType::File(FileMessageEventContent::plain(body.to_string(), uri))
    };

    Ok(RoomMessageEventContent::new(msg_type).into())
}

async fn prepare_attachment(
    attachment: &crate::gateway::channel::Attachment,
) -> ChannelResult<(Vec<u8>, String, Option<String>)> {
    let filename = attachment
        .filename
        .as_deref()
        .or_else(|| attachment.id.as_str().split('/').next_back())
        .map(String::from);

    if let Some(data) = &attachment.data {
        return Ok((data.clone(), attachment.mime_type.clone(), filename));
    }

    if let Some(url) = &attachment.url {
        if url.starts_with("http://") || url.starts_with("https://") {
            let resp = reqwest::get(url).await.map_err(|e| {
                ChannelError::ReceiveFailed(format!("Attachment download failed: {e}"))
            })?;

            if !resp.status().is_success() {
                return Err(ChannelError::ReceiveFailed(format!(
                    "Attachment download failed: {}",
                    resp.status()
                )));
            }

            let content_type = resp
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(&attachment.mime_type)
                .to_string();

            let bytes = resp.bytes().await.map_err(|e| {
                ChannelError::ReceiveFailed(format!("Attachment download body read failed: {e}"))
            })?;

            return Ok((bytes.to_vec(), content_type, filename));
        }

        if url.starts_with("mxc://") {
            return Err(ChannelError::ReceiveFailed(
                "mxc:// attachment download requires Matrix client context".to_string(),
            ));
        }
    }

    if let Some(path) = &attachment.path {
        let content = tokio::fs::read(path).await.map_err(|e| {
            ChannelError::ReceiveFailed(format!("Failed to read attachment file: {e}"))
        })?;
        return Ok((content, attachment.mime_type.clone(), filename));
    }

    Err(ChannelError::ReceiveFailed(
        "Attachment has no content (no data, url, or path)".to_string(),
    ))
}

pub async fn send_reaction(
    client: &Client,
    conversation_id: &ConversationId,
    message_id: &MessageId,
    reaction: &str,
) -> ChannelResult<()> {
    let room_id: OwnedRoomId = conversation_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid room ID: {e}")))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| ChannelError::SendFailed(format!("Room {} not found", room_id)))?;

    let event_id: OwnedEventId = message_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid event ID: {e}")))?;

    let annotation = Annotation::new(event_id, reaction.to_string());
    let content = ReactionEventContent::new(annotation);

    room.send(content)
        .await
        .map_err(|e| ChannelError::SendFailed(format!("Failed to send reaction: {e}")))?;

    Ok(())
}

pub async fn edit_message(
    client: &Client,
    conversation_id: &ConversationId,
    message_id: &MessageId,
    new_text: &str,
) -> ChannelResult<SendResult> {
    let room_id: OwnedRoomId = conversation_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid room ID: {e}")))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| ChannelError::SendFailed(format!("Room {} not found", room_id)))?;

    let event_id: OwnedEventId = message_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid event ID: {e}")))?;

    let formatted = MessageFormatter::format(new_text, MarkupFormat::Markdown);
    let chunks = MessageFormatter::split(&formatted, MATRIX_MSG_LIMIT);

    if chunks.is_empty() {
        return Err(ChannelError::SendFailed(
            "No message content to edit".to_string(),
        ));
    }

    let chunk = &chunks[0];
    let content = RoomMessageEventContent::text_html(chunk, chunk);
    let content = content.make_replacement(ReplacementMetadata::new(event_id, None));

    let response = room
        .send(content)
        .await
        .map_err(|e| ChannelError::SendFailed(format!("Failed to edit message: {e}")))?;

    Ok(SendResult {
        message_id: MessageId::new(response.event_id.to_string()),
        timestamp: Utc::now(),
    })
}

pub async fn delete_message(
    client: &Client,
    conversation_id: &ConversationId,
    message_id: &MessageId,
) -> ChannelResult<()> {
    let room_id: OwnedRoomId = conversation_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid room ID: {e}")))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| ChannelError::SendFailed(format!("Room {} not found", room_id)))?;

    let event_id: OwnedEventId = message_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid event ID: {e}")))?;

    room.redact(&event_id, None, None)
        .await
        .map_err(|e| ChannelError::SendFailed(format!("Failed to redact message: {e}")))?;

    Ok(())
}

pub async fn send_typing(
    client: &Client,
    conversation_id: &ConversationId,
    _user_id: &str,
    typing: bool,
) -> ChannelResult<()> {
    let room_id: OwnedRoomId = conversation_id
        .as_str()
        .parse()
        .map_err(|e| ChannelError::SendFailed(format!("Invalid room ID: {e}")))?;
    let room = client
        .get_room(&room_id)
        .ok_or_else(|| ChannelError::SendFailed(format!("Room {} not found", room_id)))?;

    room.typing_notice(typing)
        .await
        .map_err(|e| ChannelError::Internal(format!("Failed to send typing indicator: {e}")))?;

    Ok(())
}

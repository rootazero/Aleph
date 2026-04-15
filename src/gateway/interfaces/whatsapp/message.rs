//! Message Converter
//!
//! Converts between WhatsApp wire formats and Aleph's canonical types.

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, OutboundMessage, UserId,
};
use base64::{engine::general_purpose, Engine as _};
use chrono::TimeZone;

use super::bridge_protocol::{BridgeEvent, MediaPayload, SendRequest};

/// Convert a `BridgeEvent` into an `InboundMessage`.
pub fn bridge_message_to_inbound(
    event: &BridgeEvent,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    match event {
        BridgeEvent::Message {
            from,
            from_name,
            chat_id,
            text,
            media,
            timestamp,
            message_id,
            is_group,
            reply_to,
        } => {
            let ts = chrono::Utc
                .timestamp_opt(*timestamp, 0)
                .single()
                .unwrap_or_else(chrono::Utc::now);

            let attachments = media
                .as_ref()
                .and_then(media_payload_to_attachment)
                .into_iter()
                .collect();

            Some(InboundMessage {
                id: MessageId::new(message_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(chat_id),
                sender_id: UserId::new(from),
                sender_name: from_name.clone(),
                text: text.clone(),
                attachments,
                timestamp: ts,
                reply_to: reply_to.as_ref().map(MessageId::new),
                is_group: *is_group,
                raw: None,
                metadata: vec![],
            })
        }
        _ => None,
    }
}

/// Convert an `OutboundMessage` into a Bridge `SendRequest`.
pub fn outbound_to_send_request(message: &OutboundMessage) -> SendRequest {
    let media = message
        .attachments
        .first()
        .and_then(attachment_to_media_payload);

    SendRequest {
        to: message.conversation_id.0.clone(),
        text: message.text.clone(),
        media,
        reply_to: message.reply_to.as_ref().map(|id| id.0.clone()),
    }
}

fn media_payload_to_attachment(media: &MediaPayload) -> Option<Attachment> {
    let data = general_purpose::STANDARD.decode(&media.data).ok()?;
    Some(Attachment {
        id: String::new(),
        mime_type: media.mime_type.clone(),
        filename: media.filename.clone(),
        size: Some(data.len() as u64),
        url: None,
        path: None,
        data: Some(data),
    })
}

fn attachment_to_media_payload(attachment: &Attachment) -> Option<MediaPayload> {
    let data = attachment.data.as_ref()?;
    Some(MediaPayload {
        mime_type: attachment.mime_type.clone(),
        data: general_purpose::STANDARD.encode(data),
        filename: attachment.filename.clone(),
    })
}

pub fn wa_message_to_inbound(
    from: &str,
    from_name: Option<&str>,
    chat_id: &str,
    text: &str,
    timestamp_secs: i64,
    message_id: &str,
    is_group: bool,
    reply_to: Option<&str>,
    channel_id: &ChannelId,
) -> InboundMessage {
    let ts = chrono::Utc
        .timestamp_opt(timestamp_secs, 0)
        .single()
        .unwrap_or_else(chrono::Utc::now);

    InboundMessage {
        id: MessageId::new(message_id),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(chat_id),
        sender_id: UserId::new(from),
        sender_name: from_name.map(String::from),
        text: text.to_string(),
        attachments: vec![],
        timestamp: ts,
        reply_to: reply_to.map(MessageId::new),
        is_group,
        raw: None,
        metadata: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wa_message_to_inbound() {
        let channel_id = ChannelId::new("whatsapp");
        let msg = wa_message_to_inbound(
            "123@s.whatsapp.net",
            Some("Alice"),
            "123@s.whatsapp.net",
            "Hello",
            1708531200,
            "msg-1",
            false,
            None,
            &channel_id,
        );
        assert_eq!(msg.id.as_str(), "msg-1");
        assert_eq!(msg.text, "Hello");
        assert!(!msg.is_group);
    }
}

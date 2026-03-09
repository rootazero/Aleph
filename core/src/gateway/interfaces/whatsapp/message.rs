//! Message Converter
//!
//! Converts between the Bridge protocol types (`BridgeEvent`, `SendRequest`)
//! and Aleph's canonical message types (`InboundMessage`, `OutboundMessage`).

use base64::{engine::general_purpose, Engine as _};
use chrono::TimeZone;

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, OutboundMessage, UserId,
};

use super::bridge_protocol::{BridgeEvent, MediaPayload, SendRequest};

/// Convert a `BridgeEvent` into an `InboundMessage`.
///
/// Only the `BridgeEvent::Message` variant produces a message; all other
/// variants return `None`.
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

// ─── Internal helpers ───────────────────────────────────────────────────────

/// Decode a `MediaPayload` (base64 data) into an `Attachment`.
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

/// Encode an `Attachment` into a `MediaPayload` (base64 data).
fn attachment_to_media_payload(attachment: &Attachment) -> Option<MediaPayload> {
    let data = attachment.data.as_ref()?;
    Some(MediaPayload {
        mime_type: attachment.mime_type.clone(),
        data: general_purpose::STANDARD.encode(data),
        filename: attachment.filename.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::OutboundMessage;
    use crate::gateway::interfaces::whatsapp::bridge_protocol::MediaPayload;

    #[test]
    fn test_bridge_message_to_inbound_simple() {
        let event = BridgeEvent::Message {
            from: "1234567890@s.whatsapp.net".to_string(),
            from_name: Some("Alice".to_string()),
            chat_id: "1234567890@s.whatsapp.net".to_string(),
            text: "Hello!".to_string(),
            media: None,
            timestamp: 1708531200,
            message_id: "msg-001".to_string(),
            is_group: false,
            reply_to: None,
        };

        let channel_id = ChannelId::new("whatsapp");
        let msg = bridge_message_to_inbound(&event, &channel_id).unwrap();

        assert_eq!(msg.id.as_str(), "msg-001");
        assert_eq!(msg.channel_id.as_str(), "whatsapp");
        assert_eq!(msg.conversation_id.as_str(), "1234567890@s.whatsapp.net");
        assert_eq!(msg.sender_id.as_str(), "1234567890@s.whatsapp.net");
        assert_eq!(msg.sender_name.as_deref(), Some("Alice"));
        assert_eq!(msg.text, "Hello!");
        assert!(msg.attachments.is_empty());
        assert_eq!(msg.timestamp.timestamp(), 1708531200);
        assert!(msg.reply_to.is_none());
        assert!(!msg.is_group);
    }

    #[test]
    fn test_bridge_message_to_inbound_with_reply() {
        let event = BridgeEvent::Message {
            from: "user@s.whatsapp.net".to_string(),
            from_name: None,
            chat_id: "group@g.us".to_string(),
            text: "Replying here".to_string(),
            media: None,
            timestamp: 1708531300,
            message_id: "msg-002".to_string(),
            is_group: true,
            reply_to: Some("msg-001".to_string()),
        };

        let channel_id = ChannelId::new("whatsapp");
        let msg = bridge_message_to_inbound(&event, &channel_id).unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "msg-001");
        assert!(msg.is_group);
        assert_eq!(msg.conversation_id.as_str(), "group@g.us");
        assert!(msg.sender_name.is_none());
    }

    #[test]
    fn test_bridge_message_to_inbound_with_media() {
        // "Hello" in base64
        let b64_data = general_purpose::STANDARD.encode(b"Hello");

        let event = BridgeEvent::Message {
            from: "user@s.whatsapp.net".to_string(),
            from_name: Some("Bob".to_string()),
            chat_id: "user@s.whatsapp.net".to_string(),
            text: "".to_string(),
            media: Some(MediaPayload {
                mime_type: "image/png".to_string(),
                data: b64_data,
                filename: Some("photo.png".to_string()),
            }),
            timestamp: 1708531400,
            message_id: "msg-003".to_string(),
            is_group: false,
            reply_to: None,
        };

        let channel_id = ChannelId::new("whatsapp");
        let msg = bridge_message_to_inbound(&event, &channel_id).unwrap();

        assert_eq!(msg.attachments.len(), 1);
        let att = &msg.attachments[0];
        assert_eq!(att.mime_type, "image/png");
        assert_eq!(att.filename.as_deref(), Some("photo.png"));
        assert_eq!(att.data.as_deref(), Some(b"Hello".as_slice()));
        assert_eq!(att.size, Some(5));
    }

    #[test]
    fn test_bridge_non_message_event_returns_none() {
        let events = vec![
            BridgeEvent::Ready,
            BridgeEvent::QrExpired,
            BridgeEvent::Scanned,
            BridgeEvent::Connected {
                device_name: "Test".to_string(),
                phone_number: "+1".to_string(),
            },
            BridgeEvent::Disconnected {
                reason: "bye".to_string(),
            },
            BridgeEvent::Error {
                message: "fail".to_string(),
            },
        ];

        let channel_id = ChannelId::new("whatsapp");
        for event in &events {
            assert!(
                bridge_message_to_inbound(event, &channel_id).is_none(),
                "Expected None for {:?}",
                event
            );
        }
    }

    #[test]
    fn test_outbound_to_send_request_simple() {
        let msg = OutboundMessage::text("1234567890@s.whatsapp.net", "Hi there");
        let req = outbound_to_send_request(&msg);

        assert_eq!(req.to, "1234567890@s.whatsapp.net");
        assert_eq!(req.text, "Hi there");
        assert!(req.media.is_none());
        assert!(req.reply_to.is_none());
    }

    #[test]
    fn test_outbound_to_send_request_with_reply() {
        let msg =
            OutboundMessage::text("group@g.us", "Got it").with_reply_to("original-msg-id");
        let req = outbound_to_send_request(&msg);

        assert_eq!(req.to, "group@g.us");
        assert_eq!(req.text, "Got it");
        assert_eq!(req.reply_to.as_deref(), Some("original-msg-id"));
    }

    #[test]
    fn test_outbound_to_send_request_with_attachment() {
        let attachment = Attachment {
            id: "att-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: Some("doc.pdf".to_string()),
            size: Some(1024),
            url: None,
            path: None,
            data: Some(vec![0x25, 0x50, 0x44, 0x46]), // %PDF
        };

        let msg = OutboundMessage::text("user@s.whatsapp.net", "See attached")
            .with_attachment(attachment);
        let req = outbound_to_send_request(&msg);

        let media = req.media.as_ref().unwrap();
        assert_eq!(media.mime_type, "application/pdf");
        assert_eq!(media.filename.as_deref(), Some("doc.pdf"));

        // Verify the base64 round-trips correctly
        let decoded = general_purpose::STANDARD.decode(&media.data).unwrap();
        assert_eq!(decoded, vec![0x25, 0x50, 0x44, 0x46]);
    }
}

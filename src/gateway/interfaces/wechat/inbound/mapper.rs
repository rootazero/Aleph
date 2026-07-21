use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};

use crate::gateway::interfaces::wechat::types::{MediaRef, Message, MessageItem};

/// Extract display text from a message item.
#[must_use]
pub fn extract_text(item: &MessageItem) -> String {
    match item {
        MessageItem::Text(t) => t.text.clone(),
        MessageItem::Voice(v) => v.text.clone().unwrap_or_else(|| "[语音消息]".to_string()),
        MessageItem::Image(_) => "[图片]".to_string(),
        MessageItem::File(f) => f.file_name.clone().unwrap_or_else(|| "[文件]".to_string()),
        MessageItem::Video(_) => "[视频]".to_string(),
    }
}

fn build_media_url(media: &MediaRef) -> Option<String> {
    if let Some(url) = &media.full_url {
        return Some(url.clone());
    }
    if let Some(param) = &media.encrypt_query_param {
        let base = crate::gateway::interfaces::wechat::types::WEIXIN_CDN_BASE_URL;
        return Some(format!("{base}/download?encrypted_query_param={param}"));
    }
    None
}

/// Convert a message item to an attachment.
fn item_to_attachment(item: &MessageItem, index: usize) -> Option<Attachment> {
    match item {
        MessageItem::Image(i) => Some(Attachment {
            id: format!("img_{index}"),
            mime_type: "image/jpeg".to_string(),
            filename: None,
            size: None,
            url: build_media_url(&i.media),
            path: None,
            data: None,
        }),
        MessageItem::Voice(v) => Some(Attachment {
            id: format!("voice_{index}"),
            mime_type: "audio/amr".to_string(),
            filename: None,
            size: None,
            url: build_media_url(&v.media),
            path: None,
            data: None,
        }),
        MessageItem::File(f) => Some(Attachment {
            id: format!("file_{index}"),
            mime_type: "application/octet-stream".to_string(),
            filename: f.file_name.clone(),
            size: f.file_size,
            url: build_media_url(&f.media),
            path: None,
            data: None,
        }),
        MessageItem::Video(v) => Some(Attachment {
            id: format!("video_{index}"),
            mime_type: "video/mp4".to_string(),
            filename: None,
            size: None,
            url: build_media_url(&v.media),
            path: None,
            data: None,
        }),
        MessageItem::Text(_) => None,
    }
}

/// Guess chat type (dm or group) from message.
#[must_use]
pub fn guess_chat_type(msg: &serde_json::Value, _account_id: &str) -> (String, String) {
    let room_id = msg.get("room_id").or(msg.get("chat_room_id"));
    let from_user_id = msg
        .get("from_user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let has_room = room_id
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let is_group = has_room;

    if is_group {
        let id = room_id
            .and_then(|v| v.as_str())
            .unwrap_or(to_user_id)
            .to_string();
        ("group".to_string(), id)
    } else {
        ("dm".to_string(), from_user_id.to_string())
    }
}

/// Map an iLink message to `InboundMessage`.
#[must_use]
pub fn map_message_to_inbound(
    msg: &Message,
    channel_id: &ChannelId,
    account_id: &str,
) -> Option<InboundMessage> {
    let sender_id = UserId::new(msg.from_user_id.clone());
    let (chat_type, effective_chat_id) = {
        let msg_json = serde_json::to_value(msg).ok()?;
        guess_chat_type(&msg_json, account_id)
    };

    let conversation_id = ConversationId::new(effective_chat_id.clone());
    let is_group = chat_type == "group";

    let mut text = String::new();
    let mut attachments = Vec::new();

    for (index, item) in msg.item_list.iter().enumerate() {
        let item_text = extract_text(item);
        if !item_text.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&item_text);
        }

        if let Some(attachment) = item_to_attachment(item, index) {
            attachments.push(attachment);
        }
    }

    Some(InboundMessage {
        id: MessageId::new(msg.msg_id.clone()),
        channel_id: channel_id.clone(),
        conversation_id,
        sender_id,
        sender_name: None,
        text,
        attachments,
        timestamp: chrono::Utc::now(),
        reply_to: None,
        is_group,
        raw: serde_json::to_value(msg).ok(),
        metadata: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::wechat::types::{
        FileItem, ImageItem, MediaRef, TextItem, VideoItem, VoiceItem,
    };

    #[test]
    fn test_extract_text_from_text_item() {
        let item = MessageItem::Text(TextItem {
            text: "Hello".to_string(),
        });
        assert_eq!(extract_text(&item), "Hello");
    }

    #[test]
    fn test_extract_text_from_image_item() {
        let item = MessageItem::Image(ImageItem {
            media: MediaRef {
                encrypt_query_param: None,
                full_url: None,
                aes_key: None,
            },
            aes_key: None,
        });
        assert_eq!(extract_text(&item), "[图片]");
    }

    #[test]
    fn test_extract_text_from_voice_item_with_text() {
        let item = MessageItem::Voice(VoiceItem {
            media: MediaRef {
                encrypt_query_param: None,
                full_url: None,
                aes_key: None,
            },
            text: Some("语音转文字".to_string()),
            aes_key: None,
        });
        assert_eq!(extract_text(&item), "语音转文字");
    }

    #[test]
    fn test_extract_text_from_voice_item_without_text() {
        let item = MessageItem::Voice(VoiceItem {
            media: MediaRef {
                encrypt_query_param: None,
                full_url: None,
                aes_key: None,
            },
            text: None,
            aes_key: None,
        });
        assert_eq!(extract_text(&item), "[语音消息]");
    }

    #[test]
    fn test_extract_text_from_file_item() {
        let item = MessageItem::File(FileItem {
            file_name: Some("document.pdf".to_string()),
            file_size: Some(1024),
            media: MediaRef {
                encrypt_query_param: None,
                full_url: None,
                aes_key: None,
            },
            aes_key: None,
        });
        assert_eq!(extract_text(&item), "document.pdf");
    }

    #[test]
    fn test_extract_text_from_video_item() {
        let item = MessageItem::Video(VideoItem {
            media: MediaRef {
                encrypt_query_param: None,
                full_url: None,
                aes_key: None,
            },
            aes_key: None,
        });
        assert_eq!(extract_text(&item), "[视频]");
    }

    #[test]
    fn test_item_to_attachment_image() {
        let item = MessageItem::Image(ImageItem {
            media: MediaRef {
                encrypt_query_param: Some("abc123".to_string()),
                full_url: Some("https://example.com/img.jpg".to_string()),
                aes_key: None,
            },
            aes_key: None,
        });
        let attachment = item_to_attachment(&item, 0).unwrap();
        assert_eq!(attachment.mime_type, "image/jpeg");
        assert_eq!(
            attachment.url,
            Some("https://example.com/img.jpg".to_string())
        );
    }

    #[test]
    fn test_item_to_attachment_file() {
        let item = MessageItem::File(FileItem {
            file_name: Some("report.pdf".to_string()),
            file_size: Some(2048),
            media: MediaRef {
                encrypt_query_param: Some("xyz789".to_string()),
                full_url: None,
                aes_key: None,
            },
            aes_key: None,
        });
        let attachment = item_to_attachment(&item, 0).unwrap();
        assert_eq!(attachment.mime_type, "application/octet-stream");
        assert_eq!(attachment.filename, Some("report.pdf".to_string()));
        assert_eq!(attachment.size, Some(2048));
        assert!(attachment.url.as_ref().unwrap().contains("xyz789"));
    }

    #[test]
    fn test_map_message_to_inbound_with_media() {
        let msg = Message {
            msg_id: "msg_001".to_string(),
            from_user_id: "wxid_abc".to_string(),
            to_user_id: "bot".to_string(),
            room_id: None,
            chat_room_id: None,
            msg_type: 1,
            item_list: vec![
                MessageItem::Text(TextItem {
                    text: "Check this out".to_string(),
                }),
                MessageItem::Image(ImageItem {
                    media: MediaRef {
                        encrypt_query_param: Some("param".to_string()),
                        full_url: None,
                        aes_key: None,
                    },
                    aes_key: None,
                }),
            ],
            context_token: None,
        };
        let inbound = map_message_to_inbound(&msg, &ChannelId::new("wechat"), "bot").unwrap();

        assert_eq!(inbound.text, "Check this out\n[图片]");
        assert_eq!(inbound.attachments.len(), 1);
        assert_eq!(inbound.attachments[0].mime_type, "image/jpeg");
    }

    #[test]
    fn test_guess_chat_type_dm() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "to_user_id": "bot_id"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "dm");
        assert_eq!(id, "wxid_abc");
    }

    #[test]
    fn test_guess_chat_type_group() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "room_id": "group123"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "group");
        assert_eq!(id, "group123");
    }
}

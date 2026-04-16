use chrono::Utc;

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};
use crate::gateway::interfaces::qq::types::{
    C2CMessageEvent, GroupMessageEvent, QQAttachment, QQEvent,
};

pub fn convert_event(event: &QQEvent, channel_id: &ChannelId) -> Option<InboundMessage> {
    match event {
        QQEvent::C2CMessage(e) => Some(convert_c2c(e, channel_id)),
        QQEvent::GroupAtMessage(e) => Some(convert_group(e, channel_id)),
        _ => None,
    }
}

fn convert_c2c(event: &C2CMessageEvent, channel_id: &ChannelId) -> InboundMessage {
    let text = strip_mention(&event.content);
    InboundMessage {
        id: MessageId::new(event.id.clone()),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(format!("c2c:{}", event.author.user_openid)),
        sender_id: UserId::new(event.author.user_openid.clone()),
        sender_name: None,
        text,
        attachments: event.attachments.iter().map(convert_attachment).collect(),
        timestamp: parse_timestamp(&event.timestamp).unwrap_or_else(|_| Utc::now()),
        reply_to: event
            .message_scene
            .as_ref()
            .and_then(|s| s.ext.first().cloned().map(MessageId::new)),
        is_group: false,
        raw: Some(serde_json::to_value(event).unwrap_or_default()),
        metadata: vec![],
    }
}

fn convert_group(event: &GroupMessageEvent, channel_id: &ChannelId) -> InboundMessage {
    let text = strip_mention(&event.content);
    InboundMessage {
        id: MessageId::new(event.id.clone()),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(format!("group:{}", event.group_openid)),
        sender_id: UserId::new(event.author.member_openid.clone()),
        sender_name: None,
        text,
        attachments: event.attachments.iter().map(convert_attachment).collect(),
        timestamp: parse_timestamp(&event.timestamp).unwrap_or_else(|_| Utc::now()),
        reply_to: event
            .message_scene
            .as_ref()
            .and_then(|s| s.ext.first().cloned().map(MessageId::new)),
        is_group: true,
        raw: Some(serde_json::to_value(event).unwrap_or_default()),
        metadata: vec![],
    }
}

fn convert_attachment(att: &QQAttachment) -> Attachment {
    Attachment {
        id: att.url.clone(),
        mime_type: att.content_type.clone(),
        filename: att.filename.clone(),
        size: att.size,
        url: Some(att.url.clone()),
        path: None,
        data: None,
    }
}

fn strip_mention(content: &str) -> String {
    let mut result = content.to_string();
    loop {
        let start = result.find('<');
        let end = result.find('>');
        match (start, end) {
            (Some(s), Some(e)) if s < e => {
                let inner = &result[s + 1..e];
                if inner.starts_with('@') {
                    result.replace_range(s..=e, "");
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    result.trim().to_string()
}

fn parse_timestamp(ts: &str) -> Result<chrono::DateTime<Utc>, chrono::ParseError> {
    chrono::DateTime::parse_from_rfc3339(ts).map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_mention() {
        assert_eq!(strip_mention("<@123> hello"), "hello");
        assert_eq!(strip_mention("<@!123> hello"), "hello");
        assert_eq!(strip_mention("hello world"), "hello world");
    }

    #[test]
    fn test_parse_timestamp_rfc3339() {
        let ts = "2024-01-01T00:00:00Z";
        assert!(parse_timestamp(ts).is_ok());
    }
}

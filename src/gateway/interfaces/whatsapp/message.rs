//! Message Converter
//!
//! Converts between WhatsApp wire formats and Aleph's canonical types.

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use chrono::TimeZone;

#[allow(clippy::too_many_arguments)]
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

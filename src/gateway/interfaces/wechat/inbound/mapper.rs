//! Inbound Message Mapping
//!
//! Maps iLink messages to Aleph's InboundMessage type.

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};

use crate::gateway::interfaces::wechat::config::GroupPolicy;
use crate::gateway::interfaces::wechat::types::{Message, MessageItem, TextItem};

/// Extract text content from a message item.
pub fn extract_text(item: &MessageItem) -> String {
    match item {
        MessageItem::Text(t) => t.text.clone(),
        MessageItem::Voice(v) => v.text.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Guess chat type (dm or group) from message.
pub fn guess_chat_type(msg: &serde_json::Value, account_id: &str) -> (String, String) {
    let room_id = msg.get("room_id").or(msg.get("chat_room_id"));
    let to_user_id = msg.get("to_user_id").and_then(|v| v.as_str()).unwrap_or("");
    let from_user_id = msg
        .get("from_user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let has_room = room_id
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let is_group = has_room
        || (to_user_id == account_id
            && !from_user_id.is_empty()
            && msg.get("msg_type").and_then(|v| v.as_u64()) == Some(1));

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

/// Map an iLink message to InboundMessage.
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
    for item in &msg.item_list {
        let item_text = extract_text(item);
        if !item_text.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&item_text);
        }
    }

    Some(InboundMessage {
        id: MessageId::new(msg.msg_id.clone()),
        channel_id: channel_id.clone(),
        conversation_id,
        sender_id,
        sender_name: None,
        text,
        attachments: Vec::new(),
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

    #[test]
    fn test_extract_text_from_text_item() {
        let item = MessageItem::Text(TextItem {
            text: "Hello".to_string(),
        });
        assert_eq!(extract_text(&item), "Hello");
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

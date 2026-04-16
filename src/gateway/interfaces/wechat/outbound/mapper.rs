//! Outbound Message Mapping
//!
//! Maps OutboundMessage to iLink format.

use crate::gateway::channel::{ConversationId, OutboundMessage, UserId};

use crate::gateway::interfaces::wechat::types::{
    OutboundItem, OutboundMsg, SendMessagePayload, TextItemContent, ITEM_TEXT, MSG_STATE_FINISH,
    MSG_TYPE_BOT,
};

/// Build outbound message payload for sending.
pub fn build_send_payload(
    from_user_id: &str,
    to_user_id: &str,
    client_id: &str,
    text: &str,
    context_token: Option<&str>,
) -> SendMessagePayload {
    SendMessagePayload {
        msg: OutboundMsg {
            from_user_id: from_user_id.to_string(),
            to_user_id: to_user_id.to_string(),
            client_id: client_id.to_string(),
            message_type: MSG_TYPE_BOT,
            message_state: MSG_STATE_FINISH,
            item_list: vec![OutboundItem {
                item_type: ITEM_TEXT,
                text_item: Some(TextItemContent {
                    text: text.to_string(),
                }),
            }],
            context_token: context_token.map(String::from),
        },
    }
}

/// Map OutboundMessage to iLink send payload.
pub fn map_outbound_to_payload(
    outbound: &OutboundMessage,
    from_user_id: &str,
    to_user_id: &str,
    client_id: &str,
    context_token: Option<&str>,
) -> SendMessagePayload {
    build_send_payload(
        from_user_id,
        to_user_id,
        client_id,
        &outbound.text,
        context_token,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_send_payload() {
        let payload = build_send_payload("bot", "user1", "client1", "Hello", None);
        assert_eq!(payload.msg.from_user_id, "bot");
        assert_eq!(payload.msg.to_user_id, "user1");
        assert_eq!(payload.msg.client_id, "client1");
        assert_eq!(payload.msg.item_list[0].item_type, ITEM_TEXT);
    }

    #[test]
    fn test_build_send_payload_with_context() {
        let payload = build_send_payload("bot", "user1", "client1", "Hello", Some("ctx_123"));
        assert_eq!(payload.msg.context_token, Some("ctx_123".to_string()));
    }
}

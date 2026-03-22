use super::types::*;

/// Parse a raw WebSocket text frame into a FeishuEvent.
///
/// Returns `Ok(None)` for ping/pong frames (handled separately).
/// Returns `Ok(Some(event))` for business events.
/// Returns `Err` if the JSON is malformed.
pub fn parse_ws_frame(raw: &str) -> Result<Option<FeishuEvent>, String> {
    let envelope: WsEventEnvelope = serde_json::from_str(raw)
        .map_err(|e| format!("Failed to parse WS frame: {e}"))?;

    // Handle ping/pong frames (no header)
    if let Some(frame_type) = &envelope.frame_type {
        match frame_type.as_str() {
            "ping" | "pong" => return Ok(None),
            _ => {}
        }
    }

    let header = match &envelope.header {
        Some(h) => h,
        None => return Ok(Some(FeishuEvent::Unknown("no header".to_string()))),
    };

    let event_type = header.event_type.as_deref().unwrap_or("");

    match event_type {
        "im.message.receive_v1" => parse_message_event(&envelope),
        other => Ok(Some(FeishuEvent::Unknown(other.to_string()))),
    }
}

fn parse_message_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => return Ok(Some(FeishuEvent::Unknown("message event without body".to_string()))),
    };

    let payload: MessageEventPayload = serde_json::from_value(event_value.clone())
        .map_err(|e| format!("Failed to parse message event: {e}"))?;

    let message = payload.message.as_ref()
        .ok_or_else(|| "Missing message field".to_string())?;

    let sender_id = payload.sender.as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|sid| sid.open_id.clone())
        .unwrap_or_default();

    let chat_type = match message.chat_type.as_deref() {
        Some("p2p") => ChatType::P2p,
        _ => ChatType::Group,
    };

    let mentions = message.mentions.as_ref()
        .map(|ms| ms.iter().map(|m| Mention {
            key: m.key.clone().unwrap_or_default(),
            id: m.id.as_ref().and_then(|mid| mid.open_id.clone()).unwrap_or_default(),
            name: m.name.clone().unwrap_or_default(),
            is_bot: false, // Will be determined by comparing with bot's open_id
        }).collect())
        .unwrap_or_default();

    Ok(Some(FeishuEvent::MessageReceive {
        message_id: message.message_id.clone().unwrap_or_default(),
        chat_id: message.chat_id.clone().unwrap_or_default(),
        chat_type,
        sender_id,
        sender_name: None, // Feishu doesn't include sender name in event
        message_type: message.message_type.clone().unwrap_or_default(),
        content: message.content.clone().unwrap_or_default(),
        mentions,
        parent_id: message.parent_id.clone(),
    }))
}

/// Extract text from a Feishu message content JSON string.
///
/// For "text" type: parses `{"text": "..."}` and returns the text.
/// Removes bot mention placeholders from the text.
pub fn extract_text_content(content: &str, mentions: &[Mention]) -> Option<String> {
    let parsed: TextContent = serde_json::from_str(content).ok()?;
    let mut text = parsed.text?;

    // Remove bot mention placeholders (e.g., "@_user_1 ")
    for mention in mentions {
        if mention.is_bot {
            // Remove the placeholder and any trailing space
            text = text.replace(&format!("{} ", mention.key), "");
            text = text.replace(&mention.key, "");
        }
    }

    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Mark mentions that refer to the bot, given the bot's open_id.
pub fn mark_bot_mentions(mentions: &mut [Mention], bot_open_id: &str) {
    for mention in mentions.iter_mut() {
        if mention.id == bot_open_id {
            mention.is_bot = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ping_frame() {
        let raw = r#"{"type": "ping"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_pong_frame() {
        let raw = r#"{"type": "pong"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_unknown_event() {
        let raw = r#"{
            "header": {"event_id": "e1", "event_type": "im.chat.created_v1", "token": "t"},
            "event": {}
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::Unknown(t) => assert_eq!(t, "im.chat.created_v1"),
            _ => panic!("Expected Unknown event"),
        }
    }

    #[test]
    fn test_parse_message_receive_p2p() {
        let raw = r#"{
            "header": {
                "event_id": "evt_123",
                "event_type": "im.message.receive_v1",
                "token": "tok",
                "create_time": "1700000000"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user1"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_001",
                    "chat_id": "oc_chat1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"Hello\"}",
                    "mentions": null,
                    "parent_id": null
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                message_id, chat_id, chat_type, sender_id, message_type, content, mentions, parent_id, ..
            } => {
                assert_eq!(message_id, "msg_001");
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(chat_type, ChatType::P2p);
                assert_eq!(sender_id, "ou_user1");
                assert_eq!(message_type, "text");
                assert_eq!(content, "{\"text\":\"Hello\"}");
                assert!(mentions.is_empty());
                assert!(parent_id.is_none());
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_parse_message_receive_group_with_mention() {
        let raw = r#"{
            "header": {
                "event_id": "evt_456",
                "event_type": "im.message.receive_v1",
                "token": "tok"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user2"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_002",
                    "chat_id": "oc_group1",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 What is Rust?\"}",
                    "mentions": [
                        {"key": "@_user_1", "id": {"open_id": "ou_bot"}, "name": "Aleph"}
                    ],
                    "parent_id": "msg_001"
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                chat_type, mentions, parent_id, ..
            } => {
                assert_eq!(chat_type, ChatType::Group);
                assert_eq!(mentions.len(), 1);
                assert_eq!(mentions[0].key, "@_user_1");
                assert_eq!(mentions[0].id, "ou_bot");
                assert_eq!(mentions[0].name, "Aleph");
                assert_eq!(parent_id, Some("msg_001".to_string()));
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_extract_text_content_plain() {
        let content = r#"{"text": "Hello world"}"#;
        let result = extract_text_content(content, &[]);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_text_content_with_bot_mention() {
        let content = r#"{"text": "@_user_1 What is Rust?"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert_eq!(result, Some("What is Rust?".to_string()));
    }

    #[test]
    fn test_extract_text_content_only_mention() {
        let content = r#"{"text": "@_user_1"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert!(result.is_none());
    }

    #[test]
    fn test_mark_bot_mentions() {
        let mut mentions = vec![
            Mention { key: "@_user_1".into(), id: "ou_bot".into(), name: "Bot".into(), is_bot: false },
            Mention { key: "@_user_2".into(), id: "ou_human".into(), name: "Human".into(), is_bot: false },
        ];
        mark_bot_mentions(&mut mentions, "ou_bot");
        assert!(mentions[0].is_bot);
        assert!(!mentions[1].is_bot);
    }

    #[test]
    fn test_parse_invalid_json() {
        let raw = "not json at all";
        let result = parse_ws_frame(raw);
        assert!(result.is_err());
    }
}

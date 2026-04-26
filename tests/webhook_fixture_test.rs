use serde_json::json;

#[test]
fn test_webhook_inbound_message_parsing() {
    let json_str = include_str!("fixtures/webhook/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["sender_id"], "user-789");
    assert_eq!(data["text"], "Hello from webhook");
    assert_eq!(data["conversation_id"], "conv-456");
}

#[test]
fn test_webhook_message_to_inbound_conversion() {
    let json_str = include_str!("fixtures/webhook/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    let inbound = alephcore::gateway::channel::InboundMessage {
        id: alephcore::gateway::channel::MessageId::new(
            data["message_id"].as_str().unwrap()),
        channel_id: alephcore::gateway::channel::ChannelId::new("webhook"),
        conversation_id: alephcore::gateway::channel::ConversationId::new(
            data["conversation_id"].as_str().unwrap()),
        sender_id: alephcore::gateway::channel::UserId::new(
            data["sender_id"].as_str().unwrap()),
        sender_name: Some(data["sender_name"].as_str().unwrap().to_string()),
        text: data["text"].as_str().unwrap().to_string(),
        timestamp: chrono::DateTime::parse_from_rfc3339(data["timestamp"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc),
        attachments: vec![],
        metadata: vec![],
        reply_to: None,
        is_group: false,
        raw: Some(data.clone()),
    };

    assert_eq!(inbound.text, "Hello from webhook");
    assert_eq!(inbound.sender_id.as_str(), "user-789");
    assert_eq!(inbound.conversation_id.as_str(), "conv-456");
}

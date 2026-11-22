#[test]
fn test_push_response_fixture() {
    let fixture = include_str!("fixtures/line/push_response.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    let messages = data["sentMessages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"].as_str().unwrap(), "line-msg-123");
    assert_eq!(messages[0]["quoteToken"].as_str().unwrap(), "quote-abc");
}

#[test]
fn test_webhook_event_fixture() {
    let fixture = include_str!("fixtures/line/webhook_event.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    let events = data["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);

    let event = &events[0];
    assert_eq!(event["type"].as_str().unwrap(), "message");
    assert_eq!(event["timestamp"].as_i64().unwrap(), 1700000000000);
    assert_eq!(event["source"]["type"].as_str().unwrap(), "user");
    assert_eq!(event["source"]["userId"].as_str().unwrap(), "U123456");
    assert_eq!(event["message"]["id"].as_str().unwrap(), "line-msg-456");
    assert_eq!(event["message"]["type"].as_str().unwrap(), "text");
    assert_eq!(
        event["message"]["text"].as_str().unwrap(),
        "Hello from LINE!"
    );
}

#[test]
fn test_build_text_payload() {
    use alephcore::gateway::interfaces::line::message_ops::{
        LinePushPayload, PushMessage, TextPayload, TextPushContent,
    };

    let payload = LinePushPayload::Text(TextPayload {
        to: "U123".to_string(),
        messages: vec![PushMessage::Text(TextPushContent::new("Hello"))],
    });
    let json = serde_json::to_string(&payload).unwrap();
    assert!(json.contains("\"text\":\"Hello\""));
    assert!(json.contains("\"type\":\"text\""));
}

#[test]
fn test_build_flex_payload() {
    use alephcore::gateway::interfaces::line::message_ops::{
        FlexBubbleContents, FlexPayload, LinePushPayload,
    };

    let payload = LinePushPayload::Flex(Box::new(FlexPayload::new(
        "U123",
        "Flex message",
        FlexBubbleContents::new(),
    )));
    let json = serde_json::to_string(&payload).unwrap();
    assert!(json.contains("\"type\":\"flex\""));
    assert!(json.contains("\"altText\":\"Flex message\""));
}

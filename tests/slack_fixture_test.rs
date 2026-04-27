use alephcore::gateway::channel::ChannelId;
use alephcore::gateway::interfaces::slack::{SlackConfig, SlackMessageOps};

fn test_slack_config() -> SlackConfig {
    SlackConfig {
        app_token: "xapp-test".to_string(),
        bot_token: "xoxb-test".to_string(),
        allowed_channels: vec![],
        send_typing: true,
        dm_allowed: true,
        enable_reactions: true,
        enable_editing: true,
        enable_deletion: false,
        debounce_ms: 0,
        user_allowlist: vec![],
        resolve_user_names: false,
        directory_ttl_secs: 3600,
    }
}

#[test]
fn test_slack_event_callback_parsing() {
    let json_str = include_str!("fixtures/slack/event_callback.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["type"], "event_callback");
    assert_eq!(data["event"]["type"], "message");
    assert_eq!(data["event"]["text"], "Hello bot");
    assert_eq!(data["event"]["user"], "U123456");
    assert_eq!(data["event"]["channel"], "C12345");
    assert_eq!(data["event"]["ts"], "1234567890.123456");
}

#[test]
fn test_slack_app_mention_parsing() {
    let json_str = include_str!("fixtures/slack/app_mention.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["type"], "event_callback");
    assert_eq!(data["event"]["type"], "app_mention");
    assert_eq!(data["event"]["text"], "\u{003c}@U123456\u{003e} help me");
    assert_eq!(data["event"]["user"], "U789012");
}

#[test]
fn test_slack_event_to_inbound_conversion() {
    let json_str = include_str!("fixtures/slack/event_callback.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();
    let event = &data["event"];

    let channel_id = ChannelId::new("slack");
    let config = test_slack_config();

    let inbound = SlackMessageOps::convert_event_to_inbound(
        event,
        &channel_id,
        "B999999", // bot_user_id - different from sender
        &config,
    );

    assert!(inbound.is_some(), "convert_event_to_inbound should succeed");
    let msg = inbound.unwrap();

    assert_eq!(msg.text, "Hello bot");
    assert_eq!(msg.sender_id.as_str(), "U123456");
    assert_eq!(msg.conversation_id.as_str(), "C12345");
    assert_eq!(msg.id.as_str(), "1234567890.123456");
    assert!(msg.is_group);
    assert_eq!(msg.channel_id.as_str(), "slack");
    assert!(msg.raw.is_some());
}

#[test]
fn test_slack_event_filters_bot_messages() {
    let event = serde_json::json!({
        "type": "message",
        "text": "Hello",
        "user": "U123456",
        "ts": "1234567890.123456",
        "channel": "C12345",
        "bot_id": "B123456",
    });

    let channel_id = ChannelId::new("slack");
    let config = test_slack_config();

    let inbound =
        SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B999999", &config);

    assert!(inbound.is_none(), "Bot messages should be filtered out");
}

#[test]
fn test_slack_event_filters_own_messages() {
    let event = serde_json::json!({
        "type": "message",
        "text": "Hello",
        "user": "B999999",
        "ts": "1234567890.123456",
        "channel": "C12345",
    });

    let channel_id = ChannelId::new("slack");
    let config = test_slack_config();

    let inbound = SlackMessageOps::convert_event_to_inbound(
        &event,
        &channel_id,
        "B999999", // same as sender
        &config,
    );

    assert!(
        inbound.is_none(),
        "Bot's own messages should be filtered out"
    );
}

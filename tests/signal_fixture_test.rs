use alephcore::gateway::interfaces::signal::message_ops::SignalMessageOps;
use alephcore::gateway::interfaces::signal::SignalConfig;
use alephcore::gateway::channel::ChannelId;

#[test]
fn test_signal_fixture_inbound_message() {
    let json_str = include_str!("fixtures/signal/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["envelope"]["source"], "+9876543210");
    assert_eq!(data["envelope"]["dataMessage"]["message"], "Hello from Signal!");
}

#[test]
fn test_signal_fixture_group_message() {
    let json_str = include_str!("fixtures/signal/group_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["envelope"]["dataMessage"]["groupInfo"]["groupId"], "abc123group");
}

#[test]
fn test_signal_convert_message_from_fixture() {
    let json_str = include_str!("fixtures/signal/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    let channel_id = ChannelId::new("signal");
    let config = SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    };

    let inbound = SignalMessageOps::convert_message(
        &data,
        &channel_id,
        "+1234567890",
        &config,
    )
    .unwrap();

    assert_eq!(inbound.sender_id.as_str(), "+9876543210");
    assert_eq!(inbound.text, "Hello from Signal!");
    assert!(!inbound.is_group);
}

#[test]
fn test_signal_convert_group_from_fixture() {
    let json_str = include_str!("fixtures/signal/group_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    let channel_id = ChannelId::new("signal");
    let config = SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    };

    let inbound = SignalMessageOps::convert_message(
        &data,
        &channel_id,
        "+1234567890",
        &config,
    )
    .unwrap();

    assert!(inbound.is_group);
    assert_eq!(inbound.conversation_id.as_str(), "abc123group");
}

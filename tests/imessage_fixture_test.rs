#[test]
fn test_imessage_fixture_inbound_message() {
    let json_str = include_str!("fixtures/imessage/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["text"], "Hello from iMessage!");
    assert_eq!(data["service"], "iMessage");
}

#[test]
fn test_imessage_config_roundtrip() {
    let config = alephcore::gateway::interfaces::imessage::IMessageConfig::default();
    let json = serde_json::to_value(&config).unwrap();
    let decoded: alephcore::gateway::interfaces::imessage::IMessageConfig =
        serde_json::from_value(json).unwrap();
    assert_eq!(decoded.enabled, config.enabled);
}

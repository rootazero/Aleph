#[test]
fn test_whatsapp_fixture_inbound_message() {
    let json_str = include_str!("fixtures/whatsapp/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["text"], "Hello from WhatsApp!");
    assert_eq!(data["is_group"], false);
}

#[test]
fn test_whatsapp_config_roundtrip() {
    let config = alephcore::gateway::interfaces::whatsapp::WhatsAppConfig::default();
    let json = serde_json::to_value(&config).unwrap();
    let decoded: alephcore::gateway::interfaces::whatsapp::WhatsAppConfig =
        serde_json::from_value(json).unwrap();
    assert_eq!(decoded.phone_number, config.phone_number);
}

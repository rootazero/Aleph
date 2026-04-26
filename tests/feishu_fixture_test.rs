#[test]
fn test_feishu_fixture_inbound_message() {
    let json_str = include_str!("fixtures/feishu/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["msg_type"], "text");
    assert_eq!(data["body"]["content"], "Hello from Feishu!");
}

#[test]
fn test_feishu_config_deserialize() {
    let json = serde_json::json!({
        "app_id": "test-app",
        "app_secret": "test-secret"
    });
    let config: alephcore::gateway::interfaces::feishu::FeishuConfig =
        serde_json::from_value(json).unwrap();
    assert_eq!(config.app_id, "test-app");
    assert_eq!(config.app_secret, "test-secret");
}

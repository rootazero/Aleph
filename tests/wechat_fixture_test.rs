#[test]
fn test_wechat_fixture_inbound_message() {
    let json_str = include_str!("fixtures/wechat/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["msgtype"], "text");
    assert_eq!(data["text"]["content"], "Hello from WeChat!");
}

#[test]
fn test_wechat_config_roundtrip() {
    let config = alephcore::gateway::interfaces::wechat::WeChatConfig::default();
    let json = serde_json::to_value(&config).unwrap();
    let decoded: alephcore::gateway::interfaces::wechat::WeChatConfig =
        serde_json::from_value(json).unwrap();
    assert_eq!(decoded.account_id, config.account_id);
}

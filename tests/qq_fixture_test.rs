#[test]
fn test_qq_fixture_inbound_message() {
    let json_str = include_str!("fixtures/qq/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["author"]["username"], "TestUser");
    assert_eq!(data["content"], "Hello from QQ!");
}

#[test]
fn test_qq_config_roundtrip() {
    let config = alephcore::gateway::interfaces::qq::QQConfig {
        accounts: vec![alephcore::gateway::interfaces::qq::config::QQAccountConfig {
            id: "test".to_string(),
            app_id: "test-app".to_string(),
            client_secret: "test-secret".to_string(),
            enabled: true,
            allowed_users: vec![],
            allowed_groups: vec![],
            dm_policy: alephcore::gateway::interfaces::qq::QQDmPolicy::Open,
            group_policy: alephcore::gateway::interfaces::qq::QQGroupPolicy::Open,
        }],
    };
    let json = serde_json::to_value(&config).unwrap();
    let decoded: alephcore::gateway::interfaces::qq::QQConfig =
        serde_json::from_value(json).unwrap();
    assert_eq!(decoded.accounts.len(), config.accounts.len());
}

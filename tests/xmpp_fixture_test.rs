use alephcore::gateway::interfaces::xmpp::XmppConfig;

#[test]
fn test_xmpp_fixture_chat_message() {
    let json_str = include_str!("fixtures/xmpp/chat_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["from"], "user1@example.com");
    assert_eq!(data["body"], "Hello XMPP!");
    assert_eq!(data["type"], "chat");
}

#[test]
fn test_xmpp_fixture_muc_message() {
    let json_str = include_str!("fixtures/xmpp/muc_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["type"], "groupchat");
    assert_eq!(data["from"], "room@conference.example.com/user1");
}

#[test]
fn test_xmpp_config_serde_roundtrip() {
    let config = XmppConfig {
        jid: "bot@example.com".to_string(),
        password: "secret123".to_string(),
        server: Some("xmpp.example.com".to_string()),
        port: 5223,
        muc_rooms: vec!["room@conference.example.com".to_string()],
        use_tls: false,
        nick: "mybot".to_string(),
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: XmppConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.jid, config.jid);
    assert_eq!(deserialized.server, config.server);
    assert_eq!(deserialized.port, config.port);
}

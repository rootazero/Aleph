use alephcore::gateway::interfaces::msteams::types::Activity;

#[test]
fn test_teams_fixture_inbound_message() {
    let json_str = include_str!("fixtures/msteams/inbound_message.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["type"], "message");
    assert_eq!(data["text"], "Hello Teams!");
    assert_eq!(data["from"]["id"], "user-1");
}

#[test]
fn test_teams_fixture_send_response() {
    let json_str = include_str!("fixtures/msteams/send_response.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["id"], "resp-123");
    assert_eq!(data["conversation"]["id"], "19:conv@thread.v2");
}

#[test]
fn test_teams_activity_serde_roundtrip() {
    let activity = Activity {
        activity_type: "message".into(),
        id: Some("msg-1".into()),
        text: Some("Hello".into()),
        ..Default::default()
    };

    let json = serde_json::to_string(&activity).unwrap();
    let deserialized: Activity = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.activity_type, "message");
    assert_eq!(deserialized.id, Some("msg-1".into()));
    assert_eq!(deserialized.text, Some("Hello".into()));
}

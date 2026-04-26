use serde_json::json;

#[test]
fn test_room_message_fixture() {
    let fixture = include_str!("fixtures/matrix/room_message.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    assert_eq!(data["type"].as_str().unwrap(), "m.room.message");
    assert_eq!(data["sender"].as_str().unwrap(), "@user:example.com");
    assert_eq!(data["content"]["msgtype"].as_str().unwrap(), "m.text");
    assert_eq!(data["content"]["body"].as_str().unwrap(), "Hello from Matrix!");
    assert_eq!(data["event_id"].as_str().unwrap(), "$event-123");
    assert_eq!(data["room_id"].as_str().unwrap(), "!room:example.com");
}

#[test]
fn test_discord_message_fixture() {
    let fixture = include_str!("fixtures/discord/message.json");
    let data: serde_json::Value = serde_json::from_str(fixture).unwrap();

    assert_eq!(data["id"].as_str().unwrap(), "discord-msg-123");
    assert_eq!(data["channel_id"].as_str().unwrap(), "123456789");
    assert_eq!(data["guild_id"].as_str().unwrap(), "987654321");
    assert_eq!(data["author"]["id"].as_str().unwrap(), "111222333");
    assert_eq!(data["author"]["username"].as_str().unwrap(), "testuser");
    assert_eq!(data["author"]["bot"].as_bool().unwrap(), false);
    assert_eq!(data["content"].as_str().unwrap(), "Hello from Discord!");
}

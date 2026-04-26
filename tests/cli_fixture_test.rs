use alephcore::gateway::interfaces::cli::CliChannelConfig;

#[test]
fn test_cli_fixture_config() {
    let json_str = include_str!("fixtures/cli/config.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["prompt"], "> ");
    assert_eq!(data["username"], "testuser");
    assert_eq!(data["echo_sent"], false);
}

#[test]
fn test_cli_config_roundtrip() {
    let config = CliChannelConfig {
        id: "test-cli".to_string(),
        prompt: ">>> ".to_string(),
        username: "alice".to_string(),
        echo_sent: true,
    };

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: CliChannelConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.id, config.id);
    assert_eq!(deserialized.prompt, config.prompt);
    assert_eq!(deserialized.username, config.username);
    assert_eq!(deserialized.echo_sent, config.echo_sent);
}

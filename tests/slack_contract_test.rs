mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::slack::{SlackChannel, SlackConfig};
use common::channel_contract::test_channel_properties;

fn test_slack_config() -> SlackConfig {
    SlackConfig {
        app_token: "xapp-test".to_string(),
        bot_token: "xoxb-test".to_string(),
        allowed_channels: vec![],
        send_typing: true,
        dm_allowed: true,
        enable_reactions: true,
        enable_editing: true,
        enable_deletion: false,
        debounce_ms: 0,
        user_allowlist: vec![],
        resolve_user_names: false,
        directory_ttl_secs: 3600,
    }
}

#[test]
fn test_slack_properties() {
    let channel = SlackChannel::new("test-slack", test_slack_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "slack");
    assert!(channel.capabilities().typing_indicator);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().editing);
    assert!(channel.capabilities().rich_text);
    assert!(channel.capabilities().attachments);
    assert!(!channel.capabilities().read_receipts);
    assert_eq!(channel.capabilities().max_message_length, 3000);
}

#[tokio::test]
async fn test_slack_start_fails_with_invalid_token() {
    let mut channel = SlackChannel::new("test-slack", test_slack_config());

    // start() should fail because the test token is invalid
    let result = channel.start().await;
    assert!(result.is_err(), "start() should fail with invalid token");

    // After failure, status should be Error
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Error,
        "After failed start(), status should be Error"
    );
}

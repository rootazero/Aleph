mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::discord::{DiscordChannel, DiscordConfig};
use common::channel_contract::test_channel_properties;

fn test_discord_config() -> DiscordConfig {
    DiscordConfig {
        bot_token: "test_token_that_is_long_enough_to_pass_validation_check".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_discord_properties() {
    let channel = DiscordChannel::new("test-discord", test_discord_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "discord");
    assert!(channel.capabilities().typing_indicator);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().rich_text);
    assert!(channel.capabilities().editing);
    assert!(channel.capabilities().deletion);
    assert_eq!(channel.capabilities().max_message_length, 2000);
    assert_eq!(channel.capabilities().max_attachment_size, 25 * 1024 * 1024);
}

#[test]
fn test_discord_for_test_constructor() {
    let config = test_discord_config();
    let channel = DiscordChannel::for_test("test-discord", config);

    assert_eq!(channel.info().id.as_str(), "test-discord");
    assert_eq!(channel.channel_type(), "discord");
    assert_eq!(channel.status(), alephcore::gateway::channel::ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_discord_start_in_test_mode() {
    let mut channel = DiscordChannel::for_test("test-discord", test_discord_config());

    let result = channel.start().await;
    assert!(result.is_ok(), "start() should succeed in test mode");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected
    );
}

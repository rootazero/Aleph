mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::line::{LineChannel, LineConfig};
use common::channel_contract::test_channel_properties;

fn test_line_config() -> LineConfig {
    LineConfig {
        channel_access_token: "test-token".to_string(),
        channel_secret: "test-secret".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_line_properties() {
    let channel = LineChannel::new("test-line", test_line_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "line");
    assert!(channel.capabilities().typing_indicator);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().rich_text);
    assert!(!channel.capabilities().editing);
    assert!(channel.capabilities().deletion);
    assert_eq!(channel.capabilities().max_message_length, 5000);
    assert_eq!(channel.capabilities().max_attachment_size, 50 * 1024 * 1024);
}

#[test]
fn test_line_for_test_constructor() {
    let config = test_line_config();
    let channel = LineChannel::for_test("test-line", config.clone(), "https://mock.local");

    assert_eq!(channel.info().id.as_str(), "test-line");
    assert_eq!(channel.channel_type(), "line");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Disconnected
    );
}

#[tokio::test]
async fn test_line_start_in_test_mode() {
    let mut channel = LineChannel::for_test("test-line", test_line_config(), "https://mock.local");

    let result = channel.start().await;
    assert!(result.is_ok(), "start() should succeed in test mode");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected
    );
}

mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::cli::{CliChannel, CliChannelConfig};
use common::channel_contract::test_channel_properties;

fn test_cli_config() -> CliChannelConfig {
    CliChannelConfig {
        id: "test-cli".to_string(),
        prompt: "> ".to_string(),
        username: "testuser".to_string(),
        echo_sent: false,
    }
}

#[test]
fn test_cli_properties() {
    let channel = CliChannel::with_config(test_cli_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "cli");
    assert!(!channel.capabilities().attachments);
    assert!(!channel.capabilities().reactions);
    assert!(!channel.capabilities().rich_text);
    assert_eq!(channel.capabilities().max_message_length, 0);
}

#[tokio::test]
async fn test_cli_test_mode_start_stop() {
    let mut channel = CliChannel::for_test("test-cli");
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_cli_test_mode_send() {
    let mut channel = CliChannel::for_test("test-cli");
    channel.start().await.unwrap();

    let msg = alephcore::gateway::channel::OutboundMessage::text("cli:main", "Hello test");
    let result = channel.send(msg).await;
    assert!(result.is_ok());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_cli_test_mode_inject_and_receive() {
    let mut channel = CliChannel::for_test("test-cli");
    channel.start().await.unwrap();

    let mut rx = channel.state().take_receiver().unwrap();

    channel.inject_message("Injected message").await.unwrap();

    let received = rx.recv().await.unwrap();
    assert_eq!(received.text, "Injected message");
    assert_eq!(received.conversation_id.as_str(), "cli:main");

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_cli_send_without_start() {
    let channel = CliChannel::for_test("test-cli");
    let msg = alephcore::gateway::channel::OutboundMessage::text("cli:main", "Hello");
    let result = channel.send(msg).await;
    assert!(matches!(result, Err(alephcore::gateway::channel::ChannelError::NotConnected(_))));
}

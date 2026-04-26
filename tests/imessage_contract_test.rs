mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::imessage::{IMessageChannel, IMessageConfig};
use common::channel_contract::test_channel_properties;

fn test_imessage_config() -> IMessageConfig {
    IMessageConfig {
        enabled: true,
        ..Default::default()
    }
}

#[test]
fn test_imessage_properties() {
    let channel = IMessageChannel::new(test_imessage_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "imessage");
    assert!(channel.capabilities().images);
    assert!(channel.capabilities().audio);
    assert!(channel.capabilities().reactions);
    assert!(!channel.capabilities().replies);
    assert_eq!(channel.capabilities().max_message_length, 20000);
}

#[tokio::test]
async fn test_imessage_test_mode_start_stop() {
    let mut channel = IMessageChannel::for_test(test_imessage_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_imessage_test_mode_send() {
    let mut channel = IMessageChannel::for_test(test_imessage_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "+1234567890",
            "Hello iMessage",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_imessage_send_without_start() {
    let channel = IMessageChannel::new(test_imessage_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "+1234567890",
            "Hello",
        ))
        .await;

    assert!(result.is_err());
}

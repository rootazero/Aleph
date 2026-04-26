mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::wechat::{WeChatChannel, WeChatConfig};
use common::channel_contract::test_channel_properties;

fn test_wechat_config() -> WeChatConfig {
    WeChatConfig {
        account_id: "test-account".to_string(),
        token: "test-token".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_wechat_properties() {
    let channel = WeChatChannel::new("wechat-test", test_wechat_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "wechat");
    assert!(channel.capabilities().images);
    assert!(channel.capabilities().audio);
    assert!(channel.capabilities().typing_indicator);
    assert!(!channel.capabilities().reactions);
    assert_eq!(channel.capabilities().max_message_length, 4000);
}

#[tokio::test]
async fn test_wechat_test_mode_start_stop() {
    let mut channel = WeChatChannel::for_test("wechat-test", test_wechat_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_wechat_test_mode_send() {
    let mut channel = WeChatChannel::for_test("wechat-test", test_wechat_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user-123",
            "Hello WeChat",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_wechat_send_without_start() {
    let channel = WeChatChannel::new("wechat-test", test_wechat_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user-123",
            "Hello",
        ))
        .await;

    assert!(result.is_err());
}

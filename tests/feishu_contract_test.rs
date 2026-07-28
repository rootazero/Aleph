mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::feishu::{FeishuChannel, FeishuConfig};
use common::channel_contract::test_channel_properties;

fn test_feishu_config() -> FeishuConfig {
    let json = serde_json::json!({
        "app_id": "test-app",
        "app_secret": "test-secret"
    });
    serde_json::from_value(json).unwrap()
}

#[test]
fn test_feishu_properties() {
    let channel = FeishuChannel::new("feishu-test", test_feishu_config()).unwrap();
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "feishu");
    assert!(channel.capabilities().images);
    assert!(channel.capabilities().reactions);
    // Feishu has no typing endpoint, so `Channel::send_typing` is genuinely
    // unavailable and the bit must stay false — a capability bit is a promise to
    // implement the method, and declaring it only bought a fake success from the
    // trait default. The emulated indicator runs off `[channels.feishu]
    // typing_indicator`, a different knob.
    assert!(!channel.capabilities().typing_indicator);
    assert_eq!(channel.capabilities().max_message_length, 4096);
}

#[tokio::test]
async fn test_feishu_test_mode_start_stop() {
    let mut channel = FeishuChannel::for_test("feishu-test", test_feishu_config()).unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_feishu_test_mode_send() {
    let mut channel = FeishuChannel::for_test("feishu-test", test_feishu_config()).unwrap();
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "ou_123",
            "Hello Feishu",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_feishu_send_without_start() {
    let channel = FeishuChannel::new("feishu-test", test_feishu_config()).unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "ou_123", "Hello",
        ))
        .await;

    assert!(result.is_err());
}

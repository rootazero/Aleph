mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::qq::{QQChannel, QQConfig};
use common::channel_contract::test_channel_properties;

fn test_qq_config() -> QQConfig {
    QQConfig {
        accounts: vec![
            alephcore::gateway::interfaces::qq::config::QQAccountConfig {
                id: "test".to_string(),
                app_id: "test-app".to_string(),
                client_secret: "test-secret".to_string(),
                enabled: true,
                allowed_users: vec![],
                allowed_groups: vec![],
                dm_policy: alephcore::gateway::interfaces::qq::QQDmPolicy::Open,
                group_policy: alephcore::gateway::interfaces::qq::QQGroupPolicy::Open,
            },
        ],
    }
}

#[test]
fn test_qq_properties() {
    let channel = QQChannel::new("qq-test", test_qq_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "qq");
    assert!(channel.capabilities().images);
    assert!(channel.capabilities().replies);
    assert!(!channel.capabilities().editing);
    assert_eq!(channel.capabilities().max_message_length, 4000);
}

#[tokio::test]
async fn test_qq_test_mode_start_stop() {
    let mut channel = QQChannel::for_test("qq-test", test_qq_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_qq_test_mode_send() {
    let mut channel = QQChannel::for_test("qq-test", test_qq_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user-123", "Hello QQ",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_qq_send_without_start() {
    let channel = QQChannel::new("qq-test", test_qq_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user-123", "Hello",
        ))
        .await;

    assert!(result.is_err());
}

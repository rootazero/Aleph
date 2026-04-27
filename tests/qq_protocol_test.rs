use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::qq::{QQChannel, QQConfig};

fn test_config() -> QQConfig {
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

#[tokio::test]
async fn test_qq_protocol_send_text() {
    let mut channel = QQChannel::for_test("qq-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("user-123", "Hello QQ"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_qq_protocol_send_not_started() {
    let channel = QQChannel::new("qq-test", test_config());

    let result = channel
        .send(OutboundMessage::text("user-123", "Hello"))
        .await;
    assert!(result.is_err());
}

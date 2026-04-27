use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::feishu::{FeishuChannel, FeishuConfig};

fn test_config() -> FeishuConfig {
    let json = serde_json::json!({
        "app_id": "test-app",
        "app_secret": "test-secret"
    });
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn test_feishu_protocol_send_text() {
    let mut channel = FeishuChannel::for_test("feishu-test", test_config()).unwrap();
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("ou_123", "Hello Feishu"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_feishu_protocol_send_not_started() {
    let channel = FeishuChannel::new("feishu-test", test_config()).unwrap();

    let result = channel.send(OutboundMessage::text("ou_123", "Hello")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_feishu_protocol_react() {
    let mut channel = FeishuChannel::for_test("feishu-test", test_config()).unwrap();
    channel.start().await.unwrap();

    let result = channel
        .react(
            &alephcore::gateway::channel::ConversationId::new("ou_123"),
            &alephcore::gateway::channel::MessageId::new("msg-123"),
            "👍",
        )
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

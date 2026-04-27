use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::wechat::{WeChatChannel, WeChatConfig};

fn test_config() -> WeChatConfig {
    WeChatConfig {
        account_id: "test-account".to_string(),
        token: "test-token".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_wechat_protocol_send_text() {
    let mut channel = WeChatChannel::for_test("wechat-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("user-123", "Hello WeChat"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_wechat_protocol_send_not_started() {
    let channel = WeChatChannel::new("wechat-test", test_config());

    let result = channel
        .send(OutboundMessage::text("user-123", "Hello"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_wechat_protocol_send_typing() {
    let mut channel = WeChatChannel::for_test("wechat-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&alephcore::gateway::channel::ConversationId::new(
            "user-123",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

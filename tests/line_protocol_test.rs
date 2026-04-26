mod common;

use alephcore::gateway::channel::{Channel, ConversationId, MessageId, OutboundMessage};
use alephcore::gateway::interfaces::line::{LineChannel, LineConfig};

fn test_line_config() -> LineConfig {
    LineConfig {
        channel_access_token: "test-token".to_string(),
        channel_secret: "test-secret".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_line_send_text_message() {
    use common::mock_http::LineApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    LineApiMock::push_message(&server).await;

    let mut channel = LineChannel::for_test("test-line", test_line_config(), &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("U123", "Hello LINE!"))
        .await;

    assert!(result.is_ok(), "send() should succeed: {:?}", result.err());
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "line-msg-123");

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_line_delete_message() {
    use common::mock_http::LineApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    LineApiMock::push_message(&server).await;
    LineApiMock::delete_message(&server).await;

    let mut channel = LineChannel::for_test("test-line", test_line_config(), &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .delete(
            &ConversationId::new("U123"),
            &MessageId::new("line-msg-123".to_string()),
        )
        .await;

    assert!(result.is_ok(), "delete() should succeed: {:?}", result.err());

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_line_react_unsupported() {
    let mut channel = LineChannel::for_test("test-line", test_line_config(), "https://mock.local");
    channel.start().await.unwrap();

    let result = channel
        .react(
            &ConversationId::new("U123"),
            &MessageId::new("line-msg-123".to_string()),
            "thumbsup",
        )
        .await;

    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::UnsupportedFeature(_))),
        "LINE should not support reactions"
    );

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_line_edit_unsupported() {
    let mut channel = LineChannel::for_test("test-line", test_line_config(), "https://mock.local");
    channel.start().await.unwrap();

    let result = channel
        .edit(
            &ConversationId::new("U123"),
            &MessageId::new("line-msg-123".to_string()),
            "edited",
        )
        .await;

    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::UnsupportedFeature(_))),
        "LINE should not support editing"
    );

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_line_send_not_started_fails() {
    let channel = LineChannel::for_test("test-line", test_line_config(), "https://mock.local");

    let result = channel
        .send(OutboundMessage::text("U123", "Hello"))
        .await;

    assert!(result.is_err(), "send() should fail when not started");
}

#[tokio::test]
async fn test_line_send_typing_noop() {
    let mut channel = LineChannel::for_test("test-line", test_line_config(), "https://mock.local");
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&ConversationId::new("U123"))
        .await;

    assert!(result.is_ok(), "send_typing() should return Ok for LINE");

    let _ = channel.stop().await;
}

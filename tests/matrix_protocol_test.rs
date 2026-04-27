use alephcore::gateway::channel::{Channel, ConversationId, MessageId, OutboundMessage};
use alephcore::gateway::interfaces::matrix::MatrixChannel;

fn test_matrix_config() -> alephcore::gateway::interfaces::matrix::MatrixConfig {
    alephcore::gateway::interfaces::matrix::MatrixConfig {
        homeserver_url: "https://matrix.org".to_string(),
        access_token: "test_token".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_matrix_send_message_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("!room:example.com", "Hello Matrix!"))
        .await;

    assert!(result.is_ok(), "send() should succeed in test mode");
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "matrix-test-msg-id");
}

#[tokio::test]
async fn test_matrix_send_typing_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&ConversationId::new("!room:example.com"))
        .await;

    assert!(result.is_ok(), "send_typing() should succeed in test mode");
}

#[tokio::test]
async fn test_matrix_edit_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());
    channel.start().await.unwrap();

    let result = channel
        .edit(
            &ConversationId::new("!room:example.com"),
            &MessageId::new("$event-123".to_string()),
            "Edited text",
        )
        .await;

    assert!(result.is_ok(), "edit() should succeed in test mode");
}

#[tokio::test]
async fn test_matrix_delete_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());
    channel.start().await.unwrap();

    let result = channel
        .delete(
            &ConversationId::new("!room:example.com"),
            &MessageId::new("$event-123".to_string()),
        )
        .await;

    assert!(result.is_ok(), "delete() should succeed in test mode");
}

#[tokio::test]
async fn test_matrix_react_in_test_mode() {
    let mut channel = MatrixChannel::for_test("test-matrix", test_matrix_config());
    channel.start().await.unwrap();

    let result = channel
        .react(
            &ConversationId::new("!room:example.com"),
            &MessageId::new("$event-123".to_string()),
            "👍",
        )
        .await;

    assert!(result.is_ok(), "react() should succeed in test mode");
}

#[tokio::test]
async fn test_matrix_send_not_started_fails() {
    let channel = MatrixChannel::for_test("test-matrix", test_matrix_config());

    let result = channel
        .send(OutboundMessage::text("!room:example.com", "Hello"))
        .await;

    assert!(
        matches!(
            result,
            Err(alephcore::gateway::channel::ChannelError::NotConnected(_))
        ),
        "send() should fail with NotConnected when not started"
    );
}

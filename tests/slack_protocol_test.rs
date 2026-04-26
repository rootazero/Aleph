mod common;

use alephcore::gateway::channel::{Channel, ConversationId, MessageId, OutboundMessage};
use alephcore::gateway::interfaces::slack::{SlackChannel, SlackConfig, SlackMessageOps};
use common::mock_http::SlackApiMock;
use wiremock::MockServer;

fn test_slack_config() -> SlackConfig {
    SlackConfig {
        app_token: "xapp-test".to_string(),
        bot_token: "xoxb-test".to_string(),
        allowed_channels: vec![],
        send_typing: false, // disable to avoid spawning tasks
        dm_allowed: true,
        enable_reactions: true,
        enable_editing: true,
        enable_deletion: false,
        debounce_ms: 0,
        user_allowlist: vec![],
        resolve_user_names: false,
        directory_ttl_secs: 3600,
    }
}

#[tokio::test]
async fn test_slack_send_message_mock_api() {
    let mock_server = MockServer::start().await;
    SlackApiMock::chat_post_message(&mock_server).await;

    let channel = SlackChannel::for_test(
        "test-slack",
        test_slack_config(),
        format!("{}/api", mock_server.uri()),
    );

    let result = channel
        .send(OutboundMessage::text("C12345", "Hello Slack"))
        .await;

    assert!(result.is_ok(), "send() should succeed with mock API: {:?}", result.err());
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "1234567890.123456");
}

#[tokio::test]
async fn test_slack_send_typing_mock_api() {
    let mock_server = MockServer::start().await;
    SlackApiMock::chat_post_typing(&mock_server).await;

    let channel = SlackChannel::for_test(
        "test-slack",
        test_slack_config(),
        format!("{}/api", mock_server.uri()),
    );

    let result = channel
        .send_typing(&ConversationId::new("C12345"))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_slack_react_mock_api() {
    let mock_server = MockServer::start().await;
    SlackApiMock::reactions_add(&mock_server).await;

    let channel = SlackChannel::for_test(
        "test-slack",
        test_slack_config(),
        format!("{}/api", mock_server.uri()),
    );

    let result = channel
        .react(
            &ConversationId::new("C12345"),
            &MessageId::new("1234567890.123456"),
            "👍",
        )
        .await;

    assert!(result.is_ok(), "react() should succeed with mock API: {:?}", result.err());
}

#[tokio::test]
async fn test_slack_send_message_rate_limit() {
    let mock_server = MockServer::start().await;
    SlackApiMock::chat_post_message_rate_limit(&mock_server, 2).await;

    let channel = SlackChannel::for_test(
        "test-slack",
        test_slack_config(),
        format!("{}/api", mock_server.uri()),
    );

    let result = channel
        .send(OutboundMessage::text("C12345", "Hello"))
        .await;

    assert!(result.is_err(), "send() should fail on rate limit");
}

#[tokio::test]
async fn test_slack_validate_bot_token_mock_api() {
    let mock_server = MockServer::start().await;
    SlackApiMock::auth_test(&mock_server).await;

    let result = SlackMessageOps::validate_bot_token(
        &reqwest::Client::new(),
        "xoxb-test",
        Some(&format!("{}/api", mock_server.uri())),
    )
    .await;

    assert!(result.is_ok(), "validate_bot_token should succeed: {:?}", result.err());
    assert_eq!(result.unwrap(), "U123456");
}

mod common;

use alephcore::gateway::channel::{Channel, ConversationId, MessageId, OutboundMessage};
use alephcore::gateway::interfaces::mattermost::{MattermostChannel, MattermostConfig};

fn test_mattermost_config() -> MattermostConfig {
    MattermostConfig {
        server_url: "https://mm.example.com".to_string(),
        bot_token: "test-token".to_string(),
        allowed_channels: vec!["ch-789".to_string()],
        send_typing: false,
    }
}

#[tokio::test]
async fn test_mattermost_send_message() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::create_post(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("ch-789", "Hello Mattermost!"))
        .await;

    assert!(result.is_ok(), "send() should succeed: {:?}", result.err());
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "post-abc-123");

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_send_threaded_reply() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::create_post_with_root_id(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let mut msg = OutboundMessage::text("ch-789", "Thread reply");
    msg.reply_to = Some(MessageId::new("post-root-456".to_string()));

    let result = channel.send(msg).await;

    assert!(result.is_ok(), "send() with reply_to should succeed: {:?}", result.err());
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "post-reply-789");

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_edit_message() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::edit_post(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .edit(
            &ConversationId::new("ch-789"),
            &MessageId::new("post-abc-123".to_string()),
            "Edited message",
        )
        .await;

    assert!(result.is_ok(), "edit() should succeed: {:?}", result.err());

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_delete_message() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::delete_post(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .delete(
            &ConversationId::new("ch-789"),
            &MessageId::new("post-abc-123".to_string()),
        )
        .await;

    assert!(result.is_ok(), "delete() should succeed: {:?}", result.err());

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_react_message() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::create_reaction(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .react(
            &ConversationId::new("ch-789"),
            &MessageId::new("post-abc-123".to_string()),
            "thumbsup",
        )
        .await;

    assert!(result.is_ok(), "react() should succeed: {:?}", result.err());

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_send_typing() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;
    MattermostApiMock::typing(&server).await;

    let mut config = test_mattermost_config();
    config.send_typing = true;
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&ConversationId::new("ch-789"))
        .await;

    assert!(result.is_ok(), "send_typing() should succeed: {:?}", result.err());

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_send_not_started_fails() {
    let config = test_mattermost_config();
    let channel = MattermostChannel::for_test("test-mm", config, "https://mock.local");

    let result = channel
        .send(OutboundMessage::text("ch-789", "Hello"))
        .await;

    assert!(result.is_err(), "send() should fail when not started");
}

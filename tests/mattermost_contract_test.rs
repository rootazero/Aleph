mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::mattermost::{MattermostChannel, MattermostConfig};
use common::channel_contract::test_channel_properties;

fn test_mattermost_config() -> MattermostConfig {
    MattermostConfig {
        server_url: "https://mm.example.com".to_string(),
        bot_token: "test-token".to_string(),
        allowed_channels: vec!["ch-789".to_string()],
        send_typing: false,
    }
}

#[test]
fn test_mattermost_properties() {
    let channel = MattermostChannel::new("test-mm", test_mattermost_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "mattermost");
    assert!(channel.capabilities().typing_indicator);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().rich_text);
    assert!(channel.capabilities().editing);
    assert!(channel.capabilities().deletion);
    assert_eq!(channel.capabilities().max_message_length, 16383);
    assert_eq!(
        channel.capabilities().max_attachment_size,
        100 * 1024 * 1024
    );
}

#[test]
fn test_mattermost_for_test_constructor() {
    let config = test_mattermost_config();
    let channel = MattermostChannel::for_test("test-mm", config.clone(), "https://mock.local");

    assert_eq!(channel.info().id.as_str(), "test-mm");
    assert_eq!(channel.channel_type(), "mattermost");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Disconnected
    );
}

#[tokio::test]
async fn test_mattermost_start_with_mock_server() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());

    let result = channel.start().await;
    assert!(
        result.is_ok(),
        "start() should succeed with mock server: {:?}",
        result.err()
    );
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected
    );

    let _ = channel.stop().await;
}

#[tokio::test]
async fn test_mattermost_start_auth_failure() {
    use common::mock_http::MattermostApiMock;
    use wiremock::MockServer;

    let server = MockServer::start().await;
    MattermostApiMock::users_me_unauthorized(&server).await;

    let config = test_mattermost_config();
    let mut channel = MattermostChannel::for_test("test-mm", config, &server.uri());

    let result = channel.start().await;
    assert!(result.is_err(), "start() should fail with 401");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Error
    );
}

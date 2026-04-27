use alephcore::gateway::channel::{Channel, ChannelResult, InboundMessage, OutboundMessage};
use alephcore::gateway::interfaces::msteams::{MsTeamsChannel, MsTeamsConfig};
use alephcore::gateway::WebhookHandler;
use axum::body::Bytes;
use axum::http::HeaderMap;

fn test_config() -> MsTeamsConfig {
    MsTeamsConfig {
        app_id: "test-app-id".into(),
        app_password: "test-secret".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_teams_protocol_test_mode_start() {
    let mut channel = MsTeamsChannel::for_test("teams-test", test_config());
    let result = channel.start().await;
    assert!(result.is_ok());
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected
    );
}

#[tokio::test]
async fn test_teams_protocol_webhook_handle_message() {
    let channel = MsTeamsChannel::for_test("teams-test", test_config());

    let json_str = include_str!("fixtures/msteams/inbound_message.json");
    let body = Bytes::from(json_str.as_bytes());

    let headers = HeaderMap::new();
    let result: ChannelResult<Vec<InboundMessage>> = channel.handle(&headers, body).await;

    assert!(result.is_ok());
    let messages = result.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "Hello Teams!");
}

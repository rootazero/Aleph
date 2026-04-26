mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::msteams::{MsTeamsChannel, MsTeamsConfig};
use alephcore::gateway::WebhookHandler;
use common::channel_contract::test_channel_properties;

fn test_teams_config() -> MsTeamsConfig {
    MsTeamsConfig {
        app_id: "test-app-id".into(),
        app_password: "test-secret".into(),
        ..Default::default()
    }
}

#[test]
fn test_teams_properties() {
    let channel = MsTeamsChannel::new("teams-test", test_teams_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "msteams");
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().editing);
    assert!(channel.capabilities().deletion);
    assert!(channel.capabilities().rich_text);
    assert_eq!(channel.capabilities().max_message_length, 28_000);
}

#[tokio::test]
async fn test_teams_test_mode_start_stop() {
    let mut channel = MsTeamsChannel::for_test("teams-test", test_teams_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_teams_test_mode_webhook_verify() {
    let channel = MsTeamsChannel::for_test("teams-test", test_teams_config());

    use axum::http::HeaderMap;
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer test-token".parse().unwrap());

    assert!(channel.verify(&headers, b"{}"));
}

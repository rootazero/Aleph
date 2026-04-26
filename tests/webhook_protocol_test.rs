mod common;

use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
use common::mock_http::WebhookMock;
use wiremock::MockServer;

fn test_config(callback_url: String) -> WebhookChannelConfig {
    WebhookChannelConfig {
        secret: "test-secret".to_string(),
        callback_url,
        path: "/webhook/test".to_string(),
        allowed_senders: vec![],
    }
}

#[tokio::test]
async fn test_webhook_send_request_format() {
    let mock_server = MockServer::start().await;
    WebhookMock::callback_ok(&mock_server).await;

    let channel = WebhookChannel::with_client(
        "test-webhook",
        test_config(mock_server.uri()),
        reqwest::Client::new(),
    );

    let result = channel
        .send(OutboundMessage::text("conv-123", "Hello webhook"))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_webhook_send_with_error_response() {
    let mock_server = MockServer::start().await;
    WebhookMock::callback_error(&mock_server).await;

    let channel = WebhookChannel::with_client(
        "test-webhook",
        test_config(mock_server.uri()),
        reqwest::Client::new(),
    );

    let result = channel
        .send(OutboundMessage::text("conv-123", "Hello"))
        .await;
    assert!(result.is_err());
}

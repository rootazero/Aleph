mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
use common::channel_contract::{test_channel_contract, test_channel_properties};

fn test_webhook_config() -> WebhookChannelConfig {
    WebhookChannelConfig {
        secret: "test-secret".to_string(),
        callback_url: "http://localhost:9999/callback".to_string(),
        path: "/webhook/test".to_string(),
        allowed_senders: vec![],
    }
}

#[test]
fn test_webhook_properties() {
    let channel = WebhookChannel::new("test-webhook", test_webhook_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "webhook");
    assert!(!channel.capabilities().typing_indicator);
    assert!(!channel.capabilities().reactions);
    assert!(channel.capabilities().rich_text);
}

#[tokio::test]
async fn test_webhook_contract() {
    let channel = WebhookChannel::new("test-webhook", test_webhook_config());
    test_channel_contract(channel).await;
}

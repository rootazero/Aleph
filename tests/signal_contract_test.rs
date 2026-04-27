mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::signal::{SignalChannel, SignalConfig};
use common::channel_contract::test_channel_properties;

fn test_signal_config() -> SignalConfig {
    SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_signal_properties() {
    let channel = SignalChannel::new("signal-test", test_signal_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "signal");
    assert!(channel.capabilities().attachments);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().typing_indicator);
    assert_eq!(channel.capabilities().max_message_length, 65535);
}

#[tokio::test]
async fn test_signal_start_stop() {
    let mut channel = SignalChannel::new("signal-test", test_signal_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_signal_send_with_mock_api() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v2/send"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "timestamp": 1700000000000_i64
            })),
        )
        .mount(&mock_server)
        .await;

    let config = test_signal_config();
    let mut channel = SignalChannel::for_test("signal-test", config, mock_server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "+9876543210",
            "Hello Signal",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_signal_send_without_start() {
    let channel = SignalChannel::new("signal-test", test_signal_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "+9876543210",
            "Hello",
        ))
        .await;

    assert!(result.is_err());
}

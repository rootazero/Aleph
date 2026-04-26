use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::signal::{SignalChannel, SignalConfig};

#[tokio::test]
async fn test_signal_protocol_send_with_mock() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v2/send"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "timestamp": 1700000000000_i64
        })))
        .mount(&mock_server)
        .await;

    let config = SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    };
    let mut channel = SignalChannel::for_test("signal-test", config, mock_server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("+9876543210", "Hello Signal"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert_eq!(send_result.message_id.as_str(), "1700000000000");

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_signal_protocol_send_not_started() {
    let config = SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    };
    let channel = SignalChannel::new("signal-test", config);

    let result = channel.send(OutboundMessage::text("+9876543210", "Hello")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_signal_protocol_send_mock_error() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v2/send"))
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("Internal error"))
        .mount(&mock_server)
        .await;

    let config = SignalConfig {
        phone_number: "+1234567890".to_string(),
        ..Default::default()
    };
    let mut channel = SignalChannel::for_test("signal-test", config, mock_server.uri());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("+9876543210", "Hello Signal"))
        .await;

    assert!(result.is_err());
    channel.stop().await.unwrap();
}

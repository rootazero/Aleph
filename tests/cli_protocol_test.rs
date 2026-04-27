use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::cli::CliChannel;

#[tokio::test]
async fn test_cli_protocol_send_text() {
    let mut channel = CliChannel::for_test("test-cli");
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("cli:main", "Hello CLI"))
        .await;
    assert!(result.is_ok());

    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_cli_protocol_inject_message() {
    let mut channel = CliChannel::for_test("test-cli");
    channel.start().await.unwrap();

    let mut rx = channel.state().take_receiver().unwrap();

    channel.inject_message("Test injection").await.unwrap();

    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.text, "Test injection");
    assert_eq!(msg.sender_id.as_str(), "user");

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_cli_protocol_not_connected_fails() {
    let channel = CliChannel::for_test("test-cli");

    let result = channel
        .send(OutboundMessage::text("cli:main", "Hello"))
        .await;
    assert!(result.is_err());
}

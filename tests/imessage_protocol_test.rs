use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::imessage::{IMessageChannel, IMessageConfig};

fn test_config() -> IMessageConfig {
    IMessageConfig {
        enabled: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_imessage_protocol_send_text() {
    let mut channel = IMessageChannel::for_test(test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("+1234567890", "Hello iMessage"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_imessage_protocol_send_not_started() {
    let channel = IMessageChannel::new(test_config());

    let result = channel
        .send(OutboundMessage::text("+1234567890", "Hello"))
        .await;
    assert!(result.is_err());
}

use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::whatsapp::{WhatsAppChannel, WhatsAppConfig};

fn test_config() -> WhatsAppConfig {
    WhatsAppConfig {
        ..Default::default()
    }
}

#[tokio::test]
async fn test_whatsapp_protocol_send_text() {
    let mut channel = WhatsAppChannel::for_test("wa-test", test_config());
    channel.start().await.unwrap();

    let result = channel.send(OutboundMessage::text("1234567890@s.whatsapp.net", "Hello WA")).await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_whatsapp_protocol_send_not_started() {
    let channel = WhatsAppChannel::new("wa-test", test_config());

    let result = channel.send(OutboundMessage::text("1234567890@s.whatsapp.net", "Hello")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_whatsapp_protocol_react() {
    let mut channel = WhatsAppChannel::for_test("wa-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .react(
            &alephcore::gateway::channel::ConversationId::new("1234567890@s.whatsapp.net"),
            &alephcore::gateway::channel::MessageId::new("msg-123"),
            "👍",
        )
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

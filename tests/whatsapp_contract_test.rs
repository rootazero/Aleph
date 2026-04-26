mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::whatsapp::{WhatsAppChannel, WhatsAppConfig};
use common::channel_contract::test_channel_properties;

fn test_whatsapp_config() -> WhatsAppConfig {
    WhatsAppConfig {
        ..Default::default()
    }
}

#[test]
fn test_whatsapp_properties() {
    let channel = WhatsAppChannel::new("wa-test", test_whatsapp_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "whatsapp");
    assert!(channel.capabilities().images);
    assert!(channel.capabilities().audio);
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().replies);
    assert!(channel.capabilities().read_receipts);
    assert_eq!(channel.capabilities().max_message_length, 65536);
}

#[tokio::test]
async fn test_whatsapp_test_mode_start_stop() {
    let mut channel = WhatsAppChannel::for_test("wa-test", test_whatsapp_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_whatsapp_test_mode_send() {
    let mut channel = WhatsAppChannel::for_test("wa-test", test_whatsapp_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "1234567890@s.whatsapp.net",
            "Hello WhatsApp",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_whatsapp_send_without_start() {
    let channel = WhatsAppChannel::new("wa-test", test_whatsapp_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "1234567890@s.whatsapp.net",
            "Hello",
        ))
        .await;

    assert!(result.is_err());
}

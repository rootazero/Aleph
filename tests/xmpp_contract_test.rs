mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::xmpp::{XmppChannel, XmppConfig};
use common::channel_contract::test_channel_properties;

fn test_xmpp_config() -> XmppConfig {
    XmppConfig {
        jid: "bot@example.com".to_string(),
        password: "secret".to_string(),
        ..Default::default()
    }
}

#[test]
fn test_xmpp_properties() {
    let channel = XmppChannel::new("xmpp-test", test_xmpp_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "xmpp");
    assert!(channel.capabilities().typing_indicator);
    // The adapter implements no read-receipt method, so the bit stays false.
    assert!(!channel.capabilities().read_receipts);
    assert!(!channel.capabilities().attachments);
    assert_eq!(channel.capabilities().max_message_length, 65535);
}

#[tokio::test]
async fn test_xmpp_test_mode_start_stop() {
    let mut channel = XmppChannel::for_test("xmpp-test", test_xmpp_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_xmpp_test_mode_send() {
    let mut channel = XmppChannel::for_test("xmpp-test", test_xmpp_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user@example.com",
            "Hello XMPP",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_xmpp_send_without_start() {
    let channel = XmppChannel::for_test("xmpp-test", test_xmpp_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "user@example.com",
            "Hello",
        ))
        .await;

    assert!(result.is_err());
}

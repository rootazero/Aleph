use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::xmpp::{XmppChannel, XmppConfig};

fn test_config() -> XmppConfig {
    XmppConfig {
        jid: "bot@example.com".to_string(),
        password: "secret".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_xmpp_protocol_send_chat() {
    let mut channel = XmppChannel::for_test("xmpp-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("user@example.com", "Hello XMPP"))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_xmpp_protocol_send_groupchat() {
    let mut channel = XmppChannel::for_test(
        "xmpp-test",
        XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            muc_rooms: vec!["room@conference.example.com".to_string()],
            ..Default::default()
        },
    );
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text(
            "room@conference.example.com",
            "Hello MUC",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_xmpp_protocol_send_not_started() {
    let channel = XmppChannel::for_test("xmpp-test", test_config());

    let result = channel
        .send(OutboundMessage::text("user@example.com", "Hello"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_xmpp_protocol_typing() {
    let mut channel = XmppChannel::for_test("xmpp-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&alephcore::gateway::channel::ConversationId::new(
            "user@example.com",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

mod common;

use alephcore::gateway::channel::Channel;
use alephcore::gateway::interfaces::irc::{IrcChannel, IrcConfig};
use common::channel_contract::test_channel_properties;

fn test_irc_config() -> IrcConfig {
    IrcConfig {
        server: "irc.test.com".to_string(),
        port: 6667,
        nick: "testbot".to_string(),
        password: None,
        channels: vec!["#test".to_string()],
        use_tls: false,
        realname: "Test Bot".to_string(),
    }
}

#[test]
fn test_irc_properties() {
    let channel = IrcChannel::new("test-irc", test_irc_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "irc");
    assert!(!channel.capabilities().typing_indicator);
    assert!(!channel.capabilities().reactions);
    assert!(!channel.capabilities().attachments);
    assert!(!channel.capabilities().rich_text);
    assert_eq!(channel.capabilities().max_message_length, 400);
    assert_eq!(channel.capabilities().max_attachment_size, 0);
}

#[tokio::test]
async fn test_irc_start_in_test_mode() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());

    let result = channel.start().await;
    assert!(result.is_ok(), "start() should succeed in test mode");
    assert_eq!(
        channel.status(),
        alephcore::gateway::channel::ChannelStatus::Connected,
        "After start() in test mode, status should be Connected"
    );
}

#[tokio::test]
async fn test_irc_send_in_test_mode() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "#test",
            "Hello IRC",
        ))
        .await;

    assert!(result.is_ok(), "send() should succeed in test mode");
    let send_result = result.unwrap();
    assert!(
        send_result.message_id.as_str().starts_with("irc-sent-"),
        "message_id should start with 'irc-sent-'"
    );
}

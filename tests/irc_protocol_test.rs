use alephcore::gateway::channel::{Channel, ConversationId, OutboundMessage};
use alephcore::gateway::interfaces::irc::{IrcChannel, IrcConfig};

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

#[tokio::test]
async fn test_irc_send_message_through_channel() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("#test", "Hello IRC"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(
        send_result.message_id.as_str().starts_with("irc-sent-"),
        "message_id should start with 'irc-sent-'"
    );
}

#[tokio::test]
async fn test_irc_send_message_long_text_splits() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());
    channel.start().await.unwrap();

    let long_text = "a".repeat(500);
    let result = channel
        .send(OutboundMessage::text("#test", &long_text))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_irc_send_message_not_started_fails() {
    let channel = IrcChannel::for_test("test-irc", test_irc_config());

    let result = channel
        .send(OutboundMessage::text("#test", "Hello"))
        .await;

    assert!(result.is_err(), "send() should fail when not started");
}

#[tokio::test]
async fn test_irc_send_message_formatting() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("#test", "**bold** and *italic*"))
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_irc_send_typing_unsupported() {
    let mut channel = IrcChannel::for_test("test-irc", test_irc_config());
    channel.start().await.unwrap();

    let result = channel
        .send_typing(&ConversationId::new("#test"))
        .await;

    assert!(
        matches!(result, Err(alephcore::gateway::channel::ChannelError::UnsupportedFeature(_))),
        "IRC should not support typing indicators"
    );
}

use alephcore::gateway::channel::{Channel, OutboundMessage};
use alephcore::gateway::interfaces::nostr::{NostrChannel, NostrConfig};

const TEST_PRIVKEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn test_config() -> NostrConfig {
    NostrConfig {
        private_key: TEST_PRIVKEY.to_string(),
        relays: vec!["wss://relay.example.com".to_string()],
        ..Default::default()
    }
}

#[tokio::test]
async fn test_nostr_protocol_send_text_note() {
    let mut channel = NostrChannel::for_test("nostr-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text("public", "Hello Nostr"))
        .await;

    assert!(result.is_ok());
    let send_result = result.unwrap();
    assert!(!send_result.message_id.as_str().is_empty());

    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_nostr_protocol_send_dm() {
    let mut channel = NostrChannel::for_test("nostr-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .send(OutboundMessage::text(
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "Secret DM",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_nostr_protocol_react() {
    let mut channel = NostrChannel::for_test("nostr-test", test_config());
    channel.start().await.unwrap();

    let result = channel
        .react(
            &alephcore::gateway::channel::ConversationId::new("public"),
            &alephcore::gateway::channel::MessageId::new("event-123"),
            "+",
        )
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_nostr_protocol_send_not_started() {
    let channel = NostrChannel::new("nostr-test", test_config());

    let result = channel.send(OutboundMessage::text("public", "Hello")).await;
    assert!(result.is_err());
}

mod common;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::nostr::{NostrChannel, NostrConfig};
use common::channel_contract::test_channel_properties;

const TEST_PRIVKEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn test_nostr_config() -> NostrConfig {
    NostrConfig {
        private_key: TEST_PRIVKEY.to_string(),
        relays: vec!["wss://relay.example.com".to_string()],
        ..Default::default()
    }
}

#[test]
fn test_nostr_properties() {
    let channel = NostrChannel::new("nostr-test", test_nostr_config());
    test_channel_properties(&channel);

    assert_eq!(channel.channel_type(), "nostr");
    assert!(channel.capabilities().reactions);
    assert!(channel.capabilities().replies);
    assert!(!channel.capabilities().editing);
    assert_eq!(channel.capabilities().max_message_length, 65535);
}

#[tokio::test]
async fn test_nostr_test_mode_start_stop() {
    let mut channel = NostrChannel::for_test("nostr-test", test_nostr_config());
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);

    channel.stop().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_nostr_test_mode_send() {
    let mut channel = NostrChannel::for_test("nostr-test", test_nostr_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "public",
            "Hello Nostr",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_nostr_test_mode_send_dm() {
    let mut channel = NostrChannel::for_test("nostr-test", test_nostr_config());
    channel.start().await.unwrap();

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "Secret DM",
        ))
        .await;

    assert!(result.is_ok());
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn test_nostr_send_without_start() {
    let channel = NostrChannel::new("nostr-test", test_nostr_config());

    let result = channel
        .send(alephcore::gateway::channel::OutboundMessage::text(
            "public", "Hello",
        ))
        .await;

    assert!(result.is_err());
}

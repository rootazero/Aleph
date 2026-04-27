use alephcore::gateway::interfaces::nostr::message_ops::{
    build_dm, build_text_note, derive_pubkey, sign_event,
};
use alephcore::gateway::interfaces::nostr::NostrConfig;

const TEST_PRIVKEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn test_nostr_fixture_text_note() {
    let json_str = include_str!("fixtures/nostr/text_note.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["kind"], 1);
    assert_eq!(data["content"], "Hello Nostr!");
}

#[test]
fn test_nostr_fixture_dm_note() {
    let json_str = include_str!("fixtures/nostr/dm_note.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["kind"], 4);
    assert_eq!(data["content"], "Secret DM");
}

#[test]
fn test_nostr_build_text_note() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();
    let event = build_text_note("Hello Nostr!", &pubkey);

    assert_eq!(event.kind, 1);
    assert_eq!(event.content, "Hello Nostr!");
    assert_eq!(event.pubkey, pubkey);
}

#[test]
fn test_nostr_build_dm() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();
    let recipient = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let event = build_dm("Secret", &pubkey, recipient);

    assert_eq!(event.kind, 4);
    assert_eq!(event.content, "Secret");
}

#[test]
fn test_nostr_sign_event() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();
    let mut event = build_text_note("Test", &pubkey);

    sign_event(&mut event, TEST_PRIVKEY).unwrap();

    assert!(!event.id.is_empty());
    assert!(!event.sig.is_empty());
    assert_eq!(event.id.len(), 64);
    assert_eq!(event.sig.len(), 128);
}

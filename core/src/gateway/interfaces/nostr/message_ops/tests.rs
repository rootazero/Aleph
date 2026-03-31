//! Tests for Nostr message operations.

use super::*;
use sha2::{Digest, Sha256};

// A known test private key (NOT a real key, just deterministic for tests)
const TEST_PRIVKEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

// ==================== Event ID Tests ====================

#[test]
fn test_compute_event_id_deterministic() {
    let id1 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello Nostr");
    let id2 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello Nostr");
    assert_eq!(id1, id2, "Event ID should be deterministic");
}

#[test]
fn test_compute_event_id_is_sha256_hex() {
    let id = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello");
    assert_eq!(id.len(), 64, "SHA-256 hex should be 64 chars");
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit()),
        "Should be valid hex"
    );
}

#[test]
fn test_compute_event_id_different_content() {
    let id1 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello");
    let id2 = compute_event_id("aabbccdd", 1000000, 1, &[], "World");
    assert_ne!(id1, id2, "Different content should produce different IDs");
}

#[test]
fn test_compute_event_id_different_kind() {
    let id1 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello");
    let id2 = compute_event_id("aabbccdd", 1000000, 4, &[], "Hello");
    assert_ne!(id1, id2, "Different kind should produce different IDs");
}

#[test]
fn test_compute_event_id_different_pubkey() {
    let id1 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello");
    let id2 = compute_event_id("11223344", 1000000, 1, &[], "Hello");
    assert_ne!(id1, id2, "Different pubkey should produce different IDs");
}

#[test]
fn test_compute_event_id_different_timestamp() {
    let id1 = compute_event_id("aabbccdd", 1000000, 1, &[], "Hello");
    let id2 = compute_event_id("aabbccdd", 2000000, 1, &[], "Hello");
    assert_ne!(id1, id2, "Different timestamp should produce different IDs");
}

#[test]
fn test_compute_event_id_with_tags() {
    let tags1 = vec![vec!["p".to_string(), "deadbeef".to_string()]];
    let tags2: Vec<Vec<String>> = Vec::new();

    let id1 = compute_event_id("aabbccdd", 1000000, 4, &tags1, "Hello");
    let id2 = compute_event_id("aabbccdd", 1000000, 4, &tags2, "Hello");
    assert_ne!(id1, id2, "Different tags should produce different IDs");
}

#[test]
fn test_compute_event_id_canonical_format() {
    // Verify the canonical format: [0, pubkey, created_at, kind, tags, content]
    let pubkey = "aabbccdd";
    let created_at = 1000000u64;
    let kind = 1u64;
    let tags: Vec<Vec<String>> = vec![];
    let content = "Hello";

    let canonical = serde_json::json!([0, pubkey, created_at, kind, tags, content]);
    let serialized = serde_json::to_string(&canonical).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let expected = hex::encode(hasher.finalize());

    let actual = compute_event_id(pubkey, created_at, kind, &tags, content);
    assert_eq!(actual, expected, "Should match manual SHA-256 computation");
}

// ==================== Event Building Tests ====================

#[test]
fn test_build_text_note() {
    let pubkey = "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd";
    let event = build_text_note("Hello Nostr!", pubkey);

    assert_eq!(event.kind, 1);
    assert_eq!(event.content, "Hello Nostr!");
    assert_eq!(event.pubkey, pubkey);
    assert!(event.tags.is_empty());
    assert!(event.sig.is_empty(), "Unsigned event should have empty sig");
    assert_eq!(event.id.len(), 64, "Event ID should be SHA-256 hex");
    assert!(event.created_at > 0, "Timestamp should be set");
}

#[test]
fn test_build_dm() {
    let pubkey = "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd";
    let recipient =
        "11223344112233441122334411223344112233441122334411223344112233441122334411223344";
    let event = build_dm("Secret message", pubkey, recipient);

    assert_eq!(event.kind, 4);
    assert_eq!(event.content, "Secret message");
    assert_eq!(event.pubkey, pubkey);
    assert_eq!(event.tags.len(), 1);
    assert_eq!(event.tags[0][0], "p");
    assert_eq!(event.tags[0][1], recipient);
    assert!(event.sig.is_empty());
}

#[test]
fn test_build_reaction() {
    let pubkey = "aabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccddaabbccdd";
    let event_id = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let author = "11111111111111111111111111111111111111111111111111111111111111111111111111111111";

    let event = build_reaction("+", event_id, author, pubkey);

    assert_eq!(event.kind, 7);
    assert_eq!(event.content, "+");
    assert_eq!(event.tags.len(), 2);
    assert_eq!(event.tags[0], vec!["e", event_id]);
    assert_eq!(event.tags[1], vec!["p", author]);
}

// ==================== Subscription Message Tests ====================

#[test]
fn test_build_subscription() {
    let sub = build_subscription("sub-1", "aabbccdd", &[1, 4]);
    let parsed: serde_json::Value = serde_json::from_str(&sub).unwrap();
    let arr = parsed.as_array().unwrap();

    assert_eq!(arr[0].as_str().unwrap(), "REQ");
    assert_eq!(arr[1].as_str().unwrap(), "sub-1");

    let filter = &arr[2];
    assert_eq!(filter["kinds"], serde_json::json!([1, 4]));
    assert_eq!(filter["#p"], serde_json::json!(["aabbccdd"]));
}

#[test]
fn test_build_subscription_empty_kinds() {
    let sub = build_subscription("sub-2", "aabbccdd", &[]);
    let parsed: serde_json::Value = serde_json::from_str(&sub).unwrap();
    let arr = parsed.as_array().unwrap();
    let filter = &arr[2];
    assert_eq!(filter["kinds"], serde_json::json!([]));
}

// ==================== Event Message Tests ====================

#[test]
fn test_build_event_message() {
    let event = NostrEvent {
        id: "abc123".to_string(),
        pubkey: "pub456".to_string(),
        created_at: 1000,
        kind: 1,
        tags: vec![],
        content: "Hello".to_string(),
        sig: "sig789".to_string(),
    };

    let msg = build_event_message(&event);
    let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
    let arr = parsed.as_array().unwrap();

    assert_eq!(arr[0].as_str().unwrap(), "EVENT");
    assert_eq!(arr[1]["id"].as_str().unwrap(), "abc123");
    assert_eq!(arr[1]["content"].as_str().unwrap(), "Hello");
}

// ==================== Close Message Tests ====================

#[test]
fn test_build_close_message() {
    let msg = build_close_message("sub-1");
    let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
    let arr = parsed.as_array().unwrap();

    assert_eq!(arr[0].as_str().unwrap(), "CLOSE");
    assert_eq!(arr[1].as_str().unwrap(), "sub-1");
}

// ==================== Relay Message Parsing Tests ====================

#[test]
fn test_parse_event_message() {
    let raw = r#"["EVENT", "sub-1", {"id": "abc", "pubkey": "pub1", "created_at": 1000, "kind": 1, "tags": [], "content": "Hello", "sig": "sig1"}]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Event {
            subscription_id,
            event,
        } => {
            assert_eq!(subscription_id, "sub-1");
            assert_eq!(event.id, "abc");
            assert_eq!(event.pubkey, "pub1");
            assert_eq!(event.kind, 1);
            assert_eq!(event.content, "Hello");
        }
        _ => panic!("Expected Event message"),
    }
}

#[test]
fn test_parse_eose_message() {
    let raw = r#"["EOSE", "sub-1"]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Eose { subscription_id } => {
            assert_eq!(subscription_id, "sub-1");
        }
        _ => panic!("Expected EOSE message"),
    }
}

#[test]
fn test_parse_ok_accepted() {
    let raw = r#"["OK", "event-123", true, ""]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Ok {
            event_id,
            accepted,
            message,
        } => {
            assert_eq!(event_id, "event-123");
            assert!(accepted);
            assert_eq!(message, "");
        }
        _ => panic!("Expected OK message"),
    }
}

#[test]
fn test_parse_ok_rejected() {
    let raw = r#"["OK", "event-123", false, "duplicate: already have this event"]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Ok {
            event_id,
            accepted,
            message,
        } => {
            assert_eq!(event_id, "event-123");
            assert!(!accepted);
            assert_eq!(message, "duplicate: already have this event");
        }
        _ => panic!("Expected OK message"),
    }
}

#[test]
fn test_parse_notice_message() {
    let raw = r#"["NOTICE", "rate-limited: slow down"]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Notice { message } => {
            assert_eq!(message, "rate-limited: slow down");
        }
        _ => panic!("Expected NOTICE message"),
    }
}

#[test]
fn test_parse_empty_array() {
    let raw = r#"[]"#;
    assert!(parse_relay_message(raw).is_none());
}

#[test]
fn test_parse_invalid_json() {
    assert!(parse_relay_message("not json").is_none());
}

#[test]
fn test_parse_unknown_type() {
    let raw = r#"["UNKNOWN", "data"]"#;
    assert!(parse_relay_message(raw).is_none());
}

#[test]
fn test_parse_truncated_event() {
    // Missing event object
    let raw = r#"["EVENT", "sub-1"]"#;
    assert!(parse_relay_message(raw).is_none());
}

#[test]
fn test_parse_truncated_ok() {
    // Missing fields
    let raw = r#"["OK", "event-1"]"#;
    assert!(parse_relay_message(raw).is_none());
}

#[test]
fn test_parse_event_with_tags() {
    let raw = r#"["EVENT", "sub-1", {"id": "abc", "pubkey": "pub1", "created_at": 1000, "kind": 4, "tags": [["p", "recipient1"], ["e", "parent-event"]], "content": "DM content", "sig": "sig1"}]"#;

    let msg = parse_relay_message(raw).unwrap();
    match msg {
        RelayMessage::Event { event, .. } => {
            assert_eq!(event.kind, 4);
            assert_eq!(event.tags.len(), 2);
            assert_eq!(event.tags[0], vec!["p", "recipient1"]);
            assert_eq!(event.tags[1], vec!["e", "parent-event"]);
        }
        _ => panic!("Expected Event message"),
    }
}

// ==================== Public Key Derivation Tests ====================

#[test]
fn test_derive_pubkey_valid() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();

    // Should be 64 hex chars (32 bytes x-only pubkey)
    assert_eq!(pubkey.len(), 64, "Public key should be 64 hex chars");
    assert!(
        pubkey.chars().all(|c| c.is_ascii_hexdigit()),
        "Should be valid hex"
    );
}

#[test]
fn test_derive_pubkey_deterministic() {
    let pk1 = derive_pubkey(TEST_PRIVKEY).unwrap();
    let pk2 = derive_pubkey(TEST_PRIVKEY).unwrap();
    assert_eq!(pk1, pk2, "Same private key should produce same public key");
}

#[test]
fn test_derive_pubkey_different_keys() {
    let pk1 = derive_pubkey(TEST_PRIVKEY).unwrap();
    let pk2 =
        derive_pubkey("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210").unwrap();
    assert_ne!(
        pk1, pk2,
        "Different private keys should produce different public keys"
    );
}

#[test]
fn test_derive_pubkey_invalid_hex() {
    let result = derive_pubkey("not-hex");
    assert!(result.is_err());
}

#[test]
fn test_derive_pubkey_wrong_length() {
    let result = derive_pubkey("aabbccdd");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("32 bytes"));
}

#[test]
fn test_derive_pubkey_zero_key() {
    // All zeros is not a valid secp256k1 private key
    let result = derive_pubkey("0000000000000000000000000000000000000000000000000000000000000000");
    assert!(result.is_err());
}

// ==================== Event Conversion Tests ====================

#[test]
fn test_convert_text_note() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-1".to_string(),
        pubkey: "sender-pubkey".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![],
        content: "Hello Nostr!".to_string(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    let msg = convert_event_to_inbound(&event, &channel_id, "my-pubkey").unwrap();

    assert_eq!(msg.id.as_str(), "event-1");
    assert_eq!(msg.channel_id.as_str(), "nostr");
    assert_eq!(msg.conversation_id.as_str(), "public");
    assert_eq!(msg.sender_id.as_str(), "sender-pubkey");
    assert_eq!(msg.text, "Hello Nostr!");
    assert!(msg.is_group);
    assert!(msg.reply_to.is_none());
    assert!(msg.raw.is_some());
}

#[test]
fn test_convert_dm() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-2".to_string(),
        pubkey: "sender-pubkey".to_string(),
        created_at: 1700000000,
        kind: 4,
        tags: vec![vec!["p".to_string(), "my-pubkey".to_string()]],
        content: "Secret message".to_string(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    let msg = convert_event_to_inbound(&event, &channel_id, "my-pubkey").unwrap();

    assert!(!msg.is_group);
    assert_eq!(msg.conversation_id.as_str(), "sender-pubkey");
}

#[test]
fn test_convert_skips_own_event() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-1".to_string(),
        pubkey: "my-pubkey".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![],
        content: "My own message".to_string(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    assert!(convert_event_to_inbound(&event, &channel_id, "my-pubkey").is_none());
}

#[test]
fn test_convert_skips_empty_content() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-1".to_string(),
        pubkey: "sender-pubkey".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![],
        content: String::new(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    assert!(convert_event_to_inbound(&event, &channel_id, "my-pubkey").is_none());
}

#[test]
fn test_convert_with_reply() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-2".to_string(),
        pubkey: "sender-pubkey".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![vec!["e".to_string(), "parent-event-id".to_string()]],
        content: "Reply text".to_string(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    let msg = convert_event_to_inbound(&event, &channel_id, "my-pubkey").unwrap();

    assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "parent-event-id");
}

#[test]
fn test_convert_timestamp() {
    use crate::gateway::channel::ChannelId;

    let event = NostrEvent {
        id: "event-1".to_string(),
        pubkey: "sender-pubkey".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![],
        content: "Hello".to_string(),
        sig: "sig".to_string(),
    };

    let channel_id = ChannelId::new("nostr");
    let msg = convert_event_to_inbound(&event, &channel_id, "my-pubkey").unwrap();

    assert_eq!(msg.timestamp.timestamp(), 1700000000);
}

// ==================== Signing Tests ====================

#[test]
fn test_sign_event_produces_valid_signature() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();
    let mut event = build_text_note("Hello signed!", &pubkey);

    assert!(event.sig.is_empty());

    sign_event(&mut event, TEST_PRIVKEY).unwrap();

    // Signature should be 128 hex chars (64 bytes Schnorr signature)
    assert_eq!(
        event.sig.len(),
        128,
        "Schnorr signature should be 128 hex chars"
    );
    assert!(
        event.sig.chars().all(|c| c.is_ascii_hexdigit()),
        "Signature should be valid hex"
    );
}

#[test]
fn test_sign_event_deterministic() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();

    // Build two identical events with the same timestamp
    let created_at = 1700000000u64;
    let tags: Vec<Vec<String>> = vec![];
    let content = "Hello deterministic!";
    let id = compute_event_id(&pubkey, created_at, 1, &tags, content);

    let mut event1 = NostrEvent {
        id: id.clone(),
        pubkey: pubkey.clone(),
        created_at,
        kind: 1,
        tags: tags.clone(),
        content: content.to_string(),
        sig: String::new(),
    };

    let mut event2 = event1.clone();

    sign_event(&mut event1, TEST_PRIVKEY).unwrap();
    sign_event(&mut event2, TEST_PRIVKEY).unwrap();

    assert_eq!(
        event1.sig, event2.sig,
        "Same event + same key should produce same signature (deterministic aux_rand)"
    );
}

#[test]
fn test_sign_event_invalid_key() {
    let mut event = NostrEvent {
        id: "abc".to_string(),
        pubkey: "pub".to_string(),
        created_at: 1000,
        kind: 1,
        tags: vec![],
        content: "test".to_string(),
        sig: String::new(),
    };

    let result = sign_event(&mut event, "invalid");
    assert!(result.is_err());
}

#[test]
fn test_sign_dm_event() {
    let pubkey = derive_pubkey(TEST_PRIVKEY).unwrap();
    let recipient = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    let mut event = build_dm("Secret DM", &pubkey, recipient);

    sign_event(&mut event, TEST_PRIVKEY).unwrap();

    assert_eq!(event.sig.len(), 128);
    assert_eq!(event.kind, 4);
}

// ==================== Serde Tests ====================

#[test]
fn test_event_serde_roundtrip() {
    let event = NostrEvent {
        id: "abc123".to_string(),
        pubkey: "pub456".to_string(),
        created_at: 1700000000,
        kind: 1,
        tags: vec![vec!["p".to_string(), "target".to_string()]],
        content: "Test content".to_string(),
        sig: "sig789".to_string(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: NostrEvent = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.id, event.id);
    assert_eq!(deserialized.pubkey, event.pubkey);
    assert_eq!(deserialized.created_at, event.created_at);
    assert_eq!(deserialized.kind, event.kind);
    assert_eq!(deserialized.tags, event.tags);
    assert_eq!(deserialized.content, event.content);
    assert_eq!(deserialized.sig, event.sig);
}

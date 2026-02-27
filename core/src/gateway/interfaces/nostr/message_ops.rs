//! Nostr Protocol Operations
//!
//! Low-level NIP-01 event protocol implementation for Nostr relay communication.
//! Handles event construction, ID computation (SHA-256), relay message parsing,
//! and public key derivation from private keys using secp256k1.
//!
//! # Protocol
//!
//! Nostr uses a simple WebSocket protocol with JSON arrays:
//! - Client to Relay: `["EVENT", <event>]`, `["REQ", <sub_id>, <filter>]`, `["CLOSE", <sub_id>]`
//! - Relay to Client: `["EVENT", <sub_id>, <event>]`, `["EOSE", <sub_id>]`, `["OK", <event_id>, <bool>, <msg>]`, `["NOTICE", <msg>]`
//!
//! # Event ID Computation (NIP-01)
//!
//! The event ID is the SHA-256 hash of the canonical JSON serialization:
//! `[0, <pubkey>, <created_at>, <kind>, <tags>, <content>]`
//!
//! # Signing
//!
//! Events are signed using Schnorr signatures (BIP-340) on secp256k1.
//! Requires the `schnorr` feature on the `k256` crate.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gateway::channel::{
    ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};

/// Nostr event structure (NIP-01)
///
/// Represents a signed Nostr event that can be published to relays.
/// The `id` field is the SHA-256 of the canonical JSON serialization,
/// and `sig` is a Schnorr signature over the event ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrEvent {
    /// Event ID: SHA-256 hex of `[0, pubkey, created_at, kind, tags, content]`
    pub id: String,
    /// Author public key (hex, 32 bytes / 64 chars, x-only)
    pub pubkey: String,
    /// Unix timestamp (seconds since epoch)
    pub created_at: u64,
    /// Event kind (1 = text note, 4 = encrypted DM, 7 = reaction, etc.)
    pub kind: u64,
    /// Tags: array of string arrays (e.g., `[["p", "<pubkey>"], ["e", "<event_id>"]]`)
    pub tags: Vec<Vec<String>>,
    /// Event content (text, or encrypted payload for kind 4)
    pub content: String,
    /// Schnorr signature (hex, 64 bytes / 128 chars)
    pub sig: String,
}

/// Relay message types (relay to client)
#[derive(Debug, Clone)]
pub enum RelayMessage {
    /// Relay forwarding a subscribed event: `["EVENT", <sub_id>, <event>]`
    Event {
        subscription_id: String,
        event: NostrEvent,
    },
    /// End of stored events: `["EOSE", <sub_id>]`
    Eose {
        subscription_id: String,
    },
    /// Event acceptance result: `["OK", <event_id>, <accepted>, <message>]`
    Ok {
        event_id: String,
        accepted: bool,
        message: String,
    },
    /// Human-readable relay notice: `["NOTICE", <message>]`
    Notice {
        message: String,
    },
}

/// Compute a Nostr event ID (SHA-256 of canonical JSON).
///
/// Per NIP-01, the event ID is the SHA-256 hash of:
/// `[0, <pubkey_hex>, <created_at>, <kind>, <tags>, <content>]`
///
/// The result is a 64-character lowercase hex string.
pub fn compute_event_id(
    pubkey: &str,
    created_at: u64,
    kind: u64,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    // Build the canonical JSON array: [0, pubkey, created_at, kind, tags, content]
    let canonical = serde_json::json!([0, pubkey, created_at, kind, tags, content]);
    let serialized = serde_json::to_string(&canonical).unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let result = hasher.finalize();

    hex::encode(result)
}

/// Get current Unix timestamp in seconds.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Build a kind-1 text note event (unsigned).
///
/// Creates a public text note. The `sig` field is left empty;
/// use `sign_event()` to sign before publishing.
pub fn build_text_note(content: &str, pubkey: &str) -> NostrEvent {
    let created_at = now_unix();
    let tags: Vec<Vec<String>> = Vec::new();
    let id = compute_event_id(pubkey, created_at, 1, &tags, content);

    NostrEvent {
        id,
        pubkey: pubkey.to_string(),
        created_at,
        kind: 1,
        tags,
        content: content.to_string(),
        sig: String::new(),
    }
}

/// Build a kind-4 DM event (unsigned, plaintext).
///
/// Creates a direct message event targeting `recipient_pubkey`.
/// Per NIP-04, the content should be encrypted with AES-256-CBC,
/// but this implementation uses plaintext and notes encryption
/// as a future enhancement.
///
/// Tags include `["p", <recipient_pubkey>]` to identify the recipient.
pub fn build_dm(content: &str, pubkey: &str, recipient_pubkey: &str) -> NostrEvent {
    let created_at = now_unix();
    let tags = vec![vec!["p".to_string(), recipient_pubkey.to_string()]];
    let id = compute_event_id(pubkey, created_at, 4, &tags, content);

    NostrEvent {
        id,
        pubkey: pubkey.to_string(),
        created_at,
        kind: 4,
        tags,
        content: content.to_string(),
        sig: String::new(),
    }
}

/// Build a kind-7 reaction event (unsigned).
///
/// Creates a reaction (e.g., "+") to an existing event.
/// Tags include `["e", <event_id>]` and `["p", <author_pubkey>]`.
pub fn build_reaction(
    reaction: &str,
    event_id: &str,
    event_author_pubkey: &str,
    pubkey: &str,
) -> NostrEvent {
    let created_at = now_unix();
    let tags = vec![
        vec!["e".to_string(), event_id.to_string()],
        vec!["p".to_string(), event_author_pubkey.to_string()],
    ];
    let id = compute_event_id(pubkey, created_at, 7, &tags, reaction);

    NostrEvent {
        id,
        pubkey: pubkey.to_string(),
        created_at,
        kind: 7,
        tags,
        content: reaction.to_string(),
        sig: String::new(),
    }
}

/// Build a REQ subscription message.
///
/// Creates a subscription filter for the given kinds, optionally filtered
/// by the bot's own pubkey (to receive DMs addressed to it).
///
/// Format: `["REQ", <subscription_id>, {kinds: [...], #p: [<pubkey>]}]`
pub fn build_subscription(sub_id: &str, pubkey: &str, kinds: &[u64]) -> String {
    let filter = serde_json::json!({
        "kinds": kinds,
        "#p": [pubkey],
    });
    let msg = serde_json::json!(["REQ", sub_id, filter]);
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Build an EVENT publish message.
///
/// Format: `["EVENT", <event_json>]`
pub fn build_event_message(event: &NostrEvent) -> String {
    let msg = serde_json::json!(["EVENT", event]);
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Build a CLOSE subscription message.
///
/// Format: `["CLOSE", <subscription_id>]`
pub fn build_close_message(sub_id: &str) -> String {
    let msg = serde_json::json!(["CLOSE", sub_id]);
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Parse a relay message (EVENT, EOSE, OK, NOTICE).
///
/// Relay messages are JSON arrays with the message type as the first element.
/// Returns `None` for unrecognized or malformed messages.
pub fn parse_relay_message(msg: &str) -> Option<RelayMessage> {
    let parsed: serde_json::Value = serde_json::from_str(msg).ok()?;
    let arr = parsed.as_array()?;

    if arr.is_empty() {
        return None;
    }

    let msg_type = arr[0].as_str()?;

    match msg_type {
        "EVENT" => {
            // ["EVENT", <subscription_id>, <event>]
            if arr.len() < 3 {
                return None;
            }
            let subscription_id = arr[1].as_str()?.to_string();
            let event: NostrEvent = serde_json::from_value(arr[2].clone()).ok()?;
            Some(RelayMessage::Event {
                subscription_id,
                event,
            })
        }
        "EOSE" => {
            // ["EOSE", <subscription_id>]
            if arr.len() < 2 {
                return None;
            }
            let subscription_id = arr[1].as_str()?.to_string();
            Some(RelayMessage::Eose { subscription_id })
        }
        "OK" => {
            // ["OK", <event_id>, <accepted>, <message>]
            if arr.len() < 4 {
                return None;
            }
            let event_id = arr[1].as_str()?.to_string();
            let accepted = arr[2].as_bool()?;
            let message = arr[3].as_str().unwrap_or("").to_string();
            Some(RelayMessage::Ok {
                event_id,
                accepted,
                message,
            })
        }
        "NOTICE" => {
            // ["NOTICE", <message>]
            if arr.len() < 2 {
                return None;
            }
            let message = arr[1].as_str()?.to_string();
            Some(RelayMessage::Notice { message })
        }
        _ => None,
    }
}

/// Derive the x-only public key from a hex-encoded private key.
///
/// Uses secp256k1 scalar multiplication to derive the public key,
/// then extracts the x-coordinate only (32 bytes) as required by Nostr.
///
/// Returns a 64-character hex string of the x-only public key.
pub fn derive_pubkey(private_key_hex: &str) -> Result<String, String> {
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use k256::SecretKey;

    let privkey_bytes =
        hex::decode(private_key_hex).map_err(|e| format!("invalid hex private key: {e}"))?;

    if privkey_bytes.len() != 32 {
        return Err(format!(
            "private key must be 32 bytes, got {}",
            privkey_bytes.len()
        ));
    }

    let secret_key = SecretKey::from_slice(&privkey_bytes)
        .map_err(|e| format!("invalid secp256k1 private key: {e}"))?;

    let public_key = secret_key.public_key();
    let encoded = public_key.to_encoded_point(false); // uncompressed

    // x-only public key: take the x-coordinate (bytes 1..33 of uncompressed point)
    let x_bytes = encoded.x().ok_or("failed to extract x-coordinate")?;
    Ok(hex::encode(x_bytes))
}

/// Sign a Nostr event using Schnorr signature (BIP-340).
///
/// Computes the Schnorr signature over the event ID using the given private key
/// and sets the `sig` field on the event.
///
/// The event ID is already a SHA-256 hash (32 bytes), so we use `sign_raw`
/// (prehash signing) to sign the raw hash bytes directly, matching the
/// Nostr/BIP-340 specification.
#[cfg(feature = "nostr")]
pub fn sign_event(event: &mut NostrEvent, private_key_hex: &str) -> Result<(), String> {
    use k256::schnorr::SigningKey;

    let privkey_bytes =
        hex::decode(private_key_hex).map_err(|e| format!("invalid hex private key: {e}"))?;

    if privkey_bytes.len() != 32 {
        return Err(format!(
            "private key must be 32 bytes, got {}",
            privkey_bytes.len()
        ));
    }

    let signing_key = SigningKey::from_bytes(&privkey_bytes)
        .map_err(|e| format!("invalid signing key: {e}"))?;

    let id_bytes =
        hex::decode(&event.id).map_err(|e| format!("invalid event id hex: {e}"))?;

    // Use sign_raw with zeroed auxiliary randomness for deterministic signing.
    // The event ID is already the SHA-256 hash, so prehash signing is correct.
    let sig = signing_key
        .sign_raw(&id_bytes, &[0u8; 32])
        .map_err(|e| format!("signing failed: {e}"))?;

    event.sig = hex::encode(sig.to_bytes());

    Ok(())
}

/// Convert a Nostr event to an InboundMessage.
///
/// Maps Nostr event fields to the channel abstraction:
/// - `event.pubkey` -> `sender_id`
/// - `event.content` -> `text`
/// - `event.id` -> `id`
/// - Kind 4 (DM) -> `is_group = false`
/// - Kind 1 (text note) -> `is_group = true`
/// - `["e", <event_id>]` tag -> `reply_to`
pub fn convert_event_to_inbound(
    event: &NostrEvent,
    channel_id: &ChannelId,
    own_pubkey: &str,
) -> Option<InboundMessage> {
    // Skip own events
    if event.pubkey == own_pubkey {
        return None;
    }

    // Skip empty content
    if event.content.is_empty() {
        return None;
    }

    // Determine if this is a DM or public note
    let is_group = event.kind != 4;

    // For DMs, conversation_id is the sender's pubkey
    // For public notes, conversation_id is "public" (no specific conversation)
    let conversation_id = if event.kind == 4 {
        // DM conversation: use the sender's pubkey as conversation ID
        event.pubkey.clone()
    } else {
        // Public note: use "public" as a catch-all conversation
        "public".to_string()
    };

    // Extract reply-to from "e" tags (first "e" tag is the replied-to event)
    let reply_to = event
        .tags
        .iter()
        .find(|tag| tag.len() >= 2 && tag[0] == "e")
        .map(|tag| MessageId::new(tag[1].clone()));

    let timestamp = chrono::DateTime::from_timestamp(event.created_at as i64, 0)
        .unwrap_or_else(chrono::Utc::now);

    Some(InboundMessage {
        id: MessageId::new(event.id.clone()),
        channel_id: channel_id.clone(),
        conversation_id: ConversationId::new(conversation_id),
        sender_id: UserId::new(event.pubkey.clone()),
        sender_name: None, // Nostr doesn't have display names in events
        text: event.content.clone(),
        attachments: Vec::new(),
        timestamp,
        reply_to,
        is_group,
        raw: serde_json::to_value(event).ok(),
    })
}

/// Nostr protocol operations helper.
///
/// Provides methods for running the relay WebSocket connection loop,
/// event publishing, and subscription management.
pub struct NostrMessageOps;

impl NostrMessageOps {
    /// Run the Nostr relay WebSocket loop with reconnection.
    ///
    /// This function:
    /// 1. Connects to the first relay via WebSocket (tokio-tungstenite)
    /// 2. Sends a REQ subscription for configured event kinds
    /// 3. Reads relay messages in a select! loop
    /// 4. For EVENT messages: parses, filters by allowed_pubkeys, converts to InboundMessage
    /// 5. Handles EOSE (end of stored events) and NOTICE messages
    /// 6. Sends CLOSE on shutdown
    /// 7. Reconnects with exponential backoff on disconnection
    #[cfg(feature = "nostr")]
    pub async fn run_relay_loop(
        config: super::config::NostrConfig,
        own_pubkey: String,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut write_cmd_rx: tokio::sync::mpsc::Receiver<String>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use futures_util::{SinkExt, StreamExt};
        use std::time::Duration;

        let initial_backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(60);
        let mut backoff = initial_backoff;

        let sub_id = format!("aleph-{}", &own_pubkey[..8.min(own_pubkey.len())]);

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Connect to the first relay
            let relay_url = &config.relays[0];
            tracing::info!("Connecting to Nostr relay at {relay_url}...");

            let ws_result = tokio_tungstenite::connect_async(relay_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(
                        "Nostr relay connection failed: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            };

            backoff = initial_backoff;
            tracing::info!("Nostr relay connected to {relay_url}");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // Send subscription request
            let sub_msg = build_subscription(&sub_id, &own_pubkey, &config.subscription_kinds);
            if let Err(e) = ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(sub_msg.into()))
                .await
            {
                tracing::warn!("Nostr subscription send failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }

            tracing::info!(
                "Nostr subscribed with id={sub_id}, kinds={:?}",
                config.subscription_kinds
            );

            // Inner message loop
            let should_reconnect = 'inner: loop {
                let msg = tokio::select! {
                    msg = ws_rx.next() => msg,
                    Some(raw_cmd) = write_cmd_rx.recv() => {
                        // Outbound event publish
                        if let Err(e) = ws_tx
                            .send(tokio_tungstenite::tungstenite::Message::Text(raw_cmd.into()))
                            .await
                        {
                            tracing::warn!("Nostr write failed: {e}");
                            break 'inner true;
                        }
                        continue;
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("Nostr channel shutting down");
                            // Send CLOSE for our subscription
                            let close_msg = build_close_message(&sub_id);
                            let _ = ws_tx
                                .send(tokio_tungstenite::tungstenite::Message::Text(close_msg.into()))
                                .await;
                            let _ = ws_tx.close().await;
                            return;
                        }
                        continue;
                    }
                };

                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::warn!("Nostr WebSocket error: {e}");
                        break 'inner true;
                    }
                    None => {
                        tracing::info!("Nostr WebSocket closed");
                        break 'inner true;
                    }
                };

                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        tracing::info!("Nostr WebSocket closed by relay");
                        break 'inner true;
                    }
                    _ => continue,
                };

                // Parse relay message
                let relay_msg = match parse_relay_message(&text) {
                    Some(m) => m,
                    None => {
                        tracing::debug!("Nostr: unrecognized relay message: {}", &text[..text.len().min(100)]);
                        continue;
                    }
                };

                match relay_msg {
                    RelayMessage::Event {
                        subscription_id: _,
                        event,
                    } => {
                        // Filter by allowed pubkeys
                        if !config.is_pubkey_allowed(&event.pubkey) {
                            tracing::debug!(
                                "Nostr: ignoring event from non-allowed pubkey {}",
                                &event.pubkey[..16.min(event.pubkey.len())]
                            );
                            continue;
                        }

                        if let Some(inbound) =
                            convert_event_to_inbound(&event, &channel_id, &own_pubkey)
                        {
                            tracing::debug!(
                                "Nostr event kind={} from {}: {}",
                                event.kind,
                                &event.pubkey[..16.min(event.pubkey.len())],
                                &inbound.text[..inbound.text.len().min(50)]
                            );
                            if inbound_tx.send(inbound).await.is_err() {
                                tracing::error!("Nostr: inbound channel closed");
                                return;
                            }
                        }
                    }
                    RelayMessage::Eose { subscription_id } => {
                        tracing::debug!(
                            "Nostr: end of stored events for subscription {subscription_id}"
                        );
                    }
                    RelayMessage::Ok {
                        event_id,
                        accepted,
                        message,
                    } => {
                        if accepted {
                            tracing::debug!("Nostr: event {event_id} accepted by relay");
                        } else {
                            tracing::warn!(
                                "Nostr: event {event_id} rejected by relay: {message}"
                            );
                        }
                    }
                    RelayMessage::Notice { message } => {
                        tracing::info!("Nostr relay notice: {message}");
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Nostr: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }

        tracing::info!("Nostr relay loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A known test private key (NOT a real key, just deterministic for tests)
    const TEST_PRIVKEY: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
        let recipient = "11223344112233441122334411223344112233441122334411223344112233441122334411223344";
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
        let pk2 = derive_pubkey(
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
        )
        .unwrap();
        assert_ne!(pk1, pk2, "Different private keys should produce different public keys");
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
        let result = derive_pubkey(
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());
    }

    // ==================== Event Conversion Tests ====================

    #[test]
    fn test_convert_text_note() {
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
        let event = NostrEvent {
            id: "event-2".to_string(),
            pubkey: "sender-pubkey".to_string(),
            created_at: 1700000000,
            kind: 1,
            tags: vec![vec![
                "e".to_string(),
                "parent-event-id".to_string(),
            ]],
            content: "Reply text".to_string(),
            sig: "sig".to_string(),
        };

        let channel_id = ChannelId::new("nostr");
        let msg = convert_event_to_inbound(&event, &channel_id, "my-pubkey").unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "parent-event-id");
    }

    #[test]
    fn test_convert_timestamp() {
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
        assert_eq!(event.sig.len(), 128, "Schnorr signature should be 128 hex chars");
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
}

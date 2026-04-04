//! Nostr protocol operations: event building, signing, parsing, key derivation.

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};

use super::types::{NostrEvent, RelayMessage};

/// Compute the event ID as SHA-256 of the canonical JSON serialization.
///
/// The canonical format per NIP-01 is:
/// `[0, <pubkey>, <created_at>, <kind>, <tags>, <content>]`
pub fn compute_event_id(
    pubkey: &str,
    created_at: u64,
    kind: u64,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    let canonical = serde_json::json!([0, pubkey, created_at, kind, tags, content]);
    let serialized = serde_json::to_string(&canonical).unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hex::encode(hasher.finalize())
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

    let signing_key =
        SigningKey::from_bytes(&privkey_bytes).map_err(|e| format!("invalid signing key: {e}"))?;

    let id_bytes = hex::decode(&event.id).map_err(|e| format!("invalid event id hex: {e}"))?;

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
        metadata: vec![],
    })
}

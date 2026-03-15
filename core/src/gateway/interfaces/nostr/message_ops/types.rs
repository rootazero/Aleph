//! Nostr type definitions.

use serde::{Deserialize, Serialize};

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
    /// Event kind (1 = text note, 4 = DM, 7 = reaction, etc.)
    pub kind: u64,
    /// Array of tags, each tag is an array of strings
    pub tags: Vec<Vec<String>>,
    /// Event content (plaintext or encrypted depending on kind)
    pub content: String,
    /// Schnorr signature (BIP-340) over the event ID
    pub sig: String,
}

/// Parsed relay message types.
///
/// Represents the different message types a Nostr relay can send to a client.
#[derive(Debug)]
pub enum RelayMessage {
    /// EVENT: relay forwards an event matching a subscription
    Event {
        /// Subscription ID that matched this event
        subscription_id: String,
        /// The event itself
        event: NostrEvent,
    },
    /// EOSE: end of stored events for a subscription
    Eose {
        /// Subscription ID
        subscription_id: String,
    },
    /// OK: acknowledgment of a published event
    Ok {
        /// Event ID that was published
        event_id: String,
        /// Whether the relay accepted the event
        accepted: bool,
        /// Human-readable message (reason for rejection, etc.)
        message: String,
    },
    /// NOTICE: human-readable message from the relay
    Notice {
        /// The notice text
        message: String,
    },
}

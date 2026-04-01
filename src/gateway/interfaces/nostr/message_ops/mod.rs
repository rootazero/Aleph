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

mod ops;
mod protocol;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export public API (preserves all original exports)
pub use ops::NostrMessageOps;
pub use protocol::{
    build_close_message, build_dm, build_event_message, build_reaction, build_subscription,
    build_text_note, compute_event_id, convert_event_to_inbound, derive_pubkey,
    parse_relay_message, sign_event,
};
pub use types::{NostrEvent, RelayMessage};

//! XMPP Stanza Operations
//!
//! Low-level XMPP stanza building and parsing using simple string operations.
//! Handles only the minimal subset of XMPP XML required for messaging:
//!
//! - `<stream:stream>` — connection setup
//! - `<auth>` — SASL PLAIN authentication
//! - `<presence>` — online status + MUC join
//! - `<message>` — send/receive messages
//! - `<iq>` — info queries (ping)
//!
//! Uses raw `format!()` macros for XML building and simple string searching
//! for XML extraction. This is intentionally NOT a general XML parser.

mod ops;
mod stanza;
pub mod types;
pub(crate) mod xml_helpers;

#[cfg(test)]
mod tests;

// Re-export public API (preserves all original exports)
pub use ops::XmppMessageOps;
pub use stanza::{
    build_auth_stanza, build_bind_stanza, build_message_stanza, build_muc_join_stanza,
    build_pong_stanza, build_presence_stanza, build_session_stanza, build_stream_close,
    build_stream_header, extract_ping, extract_stanza, is_auth_failure, is_auth_success,
    is_stream_features, parse_message_stanza,
};
pub use types::{parse_jid, JidParts, XmppMessage};

/// Maximum message length for XMPP messages.
pub(crate) const XMPP_MSG_LIMIT: usize = 65535;

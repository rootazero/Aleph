//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod identity_links;
pub mod session_key;

pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};

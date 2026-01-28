//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod session_key;

pub use session_key::{DmScope, PeerKind, SessionKey};

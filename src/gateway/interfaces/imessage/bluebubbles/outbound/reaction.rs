//! Tapback reaction name → BlueBubbles associatedMessageType code.
//!
//! The mapping itself lives in the shared `imessage::reaction` module (single
//! source, also consumed by the inbound transports); this re-export keeps the
//! outbound call site's path stable.

pub use crate::gateway::interfaces::imessage::reaction::tapback_code;

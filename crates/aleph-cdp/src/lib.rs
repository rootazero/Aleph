//! Chrome DevTools Protocol transport for Aleph.
//!
//! One websocket to `/devtools/browser`, many flat sessions, a per-command timeout, and event
//! fan-out. Deliberately knows nothing about Aleph: no profiles, no engines, no tools (spec §3.1).
//! It also does not generate the CDP domain types — `methods::*` is a thin hand-written wrapper
//! over the ~30 methods Aleph actually calls, so adding a domain is a deliberate act rather than a
//! codegen side effect (R3).

mod error;
mod ids;

// The fake CDP peer other crates test against. Feature-gated so no production build carries it.
#[cfg(feature = "testkit")]
pub mod testkit;

pub use error::{CdpError, CloseReason, Result};
pub use ids::{SessionId, TargetId};

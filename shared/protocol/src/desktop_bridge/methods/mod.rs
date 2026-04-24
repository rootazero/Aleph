//! Per-method schemas for the desktop bridge JSON-RPC protocol.
//!
//! Each submodule owns:
//! - `pub const METHOD_*: &str` — canonical method names.
//! - `pub const SUGGESTED_TIMEOUT_MS*: u64` — recommended client-side timeout.
//! - Request/response structs with `serde` + `schemars::JsonSchema` derives.

pub mod ax;
pub mod bridge;
pub mod input;
pub mod media;
pub mod perm;
pub mod screen;
pub mod system;
pub mod window;

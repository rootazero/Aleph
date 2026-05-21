//! Desktop bridge — Swift helper subprocess + JSON-RPC client modules.
//!
//! Submodule layout:
//! - `codec`      : line-delimited JSON encode/decode.
//! - `inflight`   : in-flight RPC id -> oneshot reply channel table.
//! - `client`     : long-lived `SwiftBridge` RPC client driving the helper.
//! - `supervisor` : restart-on-crash backoff + disabled-mode latch.

pub mod client;
pub mod codec;
pub mod inflight;
pub mod supervisor;

//! Desktop bridge — Swift helper subprocess + JSON-RPC client modules.
//!
//! This module replaces the old `bridge.rs` single file with a focused
//! submodule layout:
//! - `codec`  : line-delimited JSON encode/decode.
//! - `inflight` : in-flight RPC id -> oneshot reply channel table.
//! - `client` (T0.5) : long-lived RPC client driving the helper subprocess.
//! - `supervisor` (T0.6) : restart-on-crash supervisor + disabled mode.
//!
//! The legacy spawn-per-call `SwiftBridge` below is kept for one release so
//! `desktop/macos/src/pim.rs` continues to compile; it is replaced by the
//! new long-lived client in Task 0.5.

pub mod codec;
pub mod inflight;

pub mod client;

#[allow(dead_code)] // populated in T0.6
pub mod supervisor;

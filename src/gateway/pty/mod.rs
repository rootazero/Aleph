//! Embedded PTY terminal subsystem.
//!
//! Gives the gateway an interactive pseudo-terminal capability — the Rust
//! mapping of hermes-agent's `pty_bridge.py` embedded-terminal stream. Output
//! is multiplexed over the *existing* loopback `/ws` JSON-RPC transport via the
//! `pty.output` topic instead of a second WebSocket/ephemeral port, so the
//! desktop shell's single fixed-port discovery + bootstrap-cookie auth model is
//! preserved (R6 one core, many channels).
//!
//! Layers:
//! - [`session`]: a single PTY (`portable-pty` master/child) + reader thread.
//! - [`manager`]: the process-global bounded session registry + event-bus sink.
//!
//! Handlers live in `gateway::handlers::pty` (`pty.spawn/input/resize/close/list`).
//! Under the LAN-trust model every connection is the implicit owner/operator,
//! so the PTY surface is open to all connections.

pub mod manager;
pub mod screen;
pub mod session;

pub use manager::{attach_event_bus, manager, PtyManager, SessionInfo, SpawnResult};
pub use session::{PtySession, SpawnOptions};

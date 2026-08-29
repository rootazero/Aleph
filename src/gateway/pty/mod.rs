//! Embedded PTY terminal subsystem.
//!
//! Gives the gateway an interactive pseudo-terminal capability — the Rust
//! mapping of hermes-agent's `pty_bridge.py` embedded-terminal stream. Output
//! is multiplexed over the *existing* loopback `/ws` JSON-RPC transport via the
//! `pty.screen` topic instead of a second WebSocket/ephemeral port, so the
//! desktop shell's single fixed-port discovery + bootstrap-cookie auth model is
//! preserved (R6 one core, many channels).
//!
//! Layers:
//! - [`session`]: a single PTY (`portable-pty` master/child) + reader thread.
//! - [`manager`]: the process-global bounded session registry + event-bus sink.
//!
//! Handlers live in `gateway::handlers::pty` (`pty.spawn/input/resize/close/list/attach`).
//! Operator-only, on both the RPC and event faces — see the module doc on
//! `gateway::handlers::pty` for why both faces matter and how the sentence
//! that used to stand here (claiming the surface was open to all connections)
//! survived one face going admin-only while the other didn't.

pub mod manager;
pub mod screen;
pub mod session;

pub use manager::{attach_event_bus, manager, PtyManager, SessionInfo, SpawnResult};
pub use session::{PtySession, SpawnOptions};

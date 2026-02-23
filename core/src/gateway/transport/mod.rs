//! Transport layer for bridge process IPC.
//!
//! Provides a platform-independent abstraction ([`Transport`]) over
//! various IPC mechanisms used to communicate with external bridge
//! processes (Signal, WhatsApp, etc.).
//!
//! # Available transports
//!
//! - [`unix_socket::UnixSocketTransport`] — JSON-RPC 2.0 over Unix domain sockets.

mod traits;
pub mod unix_socket;

pub use traits::*;

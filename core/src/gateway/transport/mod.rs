//! Transport layer for bridge process IPC.
//!
//! Provides a platform-independent abstraction ([`Transport`]) over
//! various IPC mechanisms used to communicate with external bridge
//! processes (Signal, WhatsApp, etc.).
//!
//! # Available transports
//!
//! - [`unix_socket::UnixSocketTransport`] — JSON-RPC 2.0 over Unix domain sockets.
//! - [`stdio::StdioTransport`] — JSON-RPC 2.0 over stdin/stdout pipes.

mod traits;
pub mod stdio;
#[cfg(unix)]
pub mod unix_socket;

pub use stdio::StdioTransport;
pub use traits::*;

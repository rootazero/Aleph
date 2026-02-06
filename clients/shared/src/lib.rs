//! Aleph Client SDK
//!
//! Shared logic for Rust-based Aleph clients (CLI, Desktop, etc.)
//!
//! ## Features
//!
//! - `transport`: WebSocket connection management
//! - `rpc`: JSON-RPC 2.0 protocol handling
//! - `client`: Complete client instance (requires `transport` + `rpc`)
//! - `local-executor`: Local tool execution trait and utilities
//! - `native-tls`: Native TLS support (default)
//! - `rustls`: Pure Rust TLS support
//! - `tracing`: Optional logging support
//!
//! ## Example
//!
//! ```no_run
//! use aleph_client_sdk::{GatewayClient, ConfigStore};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = GatewayClient::new("ws://127.0.0.1:18789");
//!     // Use client...
//!     Ok(())
//! }
//! ```

// Public modules
mod error;
pub use error::{ClientError, Result};

#[cfg(feature = "transport")]
mod transport;
#[cfg(feature = "transport")]
pub use transport::{
    Transport, ConnectionState, TransportMessage,
    WsStream, WsWriter, WsReader, read_messages,
};

#[cfg(feature = "rpc")]
mod rpc;
#[cfg(feature = "rpc")]
pub use rpc::RpcClient;

#[cfg(feature = "client")]
mod auth;
#[cfg(feature = "client")]
pub use auth::{ConfigStore, AuthToken};

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::GatewayClient;

#[cfg(feature = "local-executor")]
mod executor;
#[cfg(feature = "local-executor")]
pub use executor::LocalExecutor;

// Re-export protocol types
pub use aleph_protocol::{
    ClientManifest, ClientCapabilities, ClientEnvironment,
    ExecutionConstraints, JsonRpcRequest, JsonRpcResponse,
    StreamEvent,
};

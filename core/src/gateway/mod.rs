//! WebSocket Gateway for Moltbot-style architecture
//!
//! Provides a centralized control plane for all agent interactions.
//! The Gateway acts as a WebSocket server that clients connect to for
//! sending commands and receiving events using JSON-RPC 2.0 protocol.
//!
//! # Features
//!
//! - **JSON-RPC 2.0**: Standard request/response protocol
//! - **Event Broadcasting**: Push events to all connected clients
//! - **Bearer Token Auth**: Secure connection authentication
//! - **Device Pairing**: QR code / PIN-based pairing flow
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::gateway::GatewayServer;
//! use std::net::SocketAddr;
//!
//! let addr: SocketAddr = "127.0.0.1:18789".parse().unwrap();
//! let server = GatewayServer::new(addr);
//! server.run().await?;
//! ```

#[cfg(feature = "gateway")]
pub mod protocol;
#[cfg(feature = "gateway")]
pub mod server;
#[cfg(feature = "gateway")]
pub mod event_bus;
#[cfg(feature = "gateway")]
pub mod event_emitter;
#[cfg(feature = "gateway")]
pub mod router;
#[cfg(feature = "gateway")]
pub mod security;
#[cfg(feature = "gateway")]
pub mod handlers;

#[cfg(feature = "gateway")]
pub use server::GatewayServer;
#[cfg(feature = "gateway")]
pub use protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError};
#[cfg(feature = "gateway")]
pub use event_bus::GatewayEventBus;
#[cfg(feature = "gateway")]
pub use event_emitter::{EventEmitter, StreamEvent, GatewayEventEmitter, NoOpEventEmitter};
#[cfg(feature = "gateway")]
pub use router::AgentRouter;

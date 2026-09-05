//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod config;
mod connection;
mod error;
mod gateway_client;
mod session_resolve;
mod tls;

pub use config::{CliConfig, ManifestConfig};
pub use connection::{AlephClient, TopicEvent};
pub use error::{CliError, CliResult};
pub use gateway_client::{GatewayClient, DEFAULT_GATEWAY_URL, DEFAULT_TIMEOUT_MS};
pub use session_resolve::resolve_last_session;

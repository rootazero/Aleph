//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod config;
mod connection;
mod credential;
mod credential_store;
mod error;
mod gateway_client;
mod session_resolve;
mod tls;

pub use config::{CliConfig, ManifestConfig};
pub use connection::{AlephClient, StreamContinuity, TopicEvent};
pub use credential::{DeviceCredential, VerifiedIdentity};
pub use credential_store::DeviceCredentialStore;
pub use error::{CliError, CliResult};
pub use gateway_client::{GatewayClient, DEFAULT_GATEWAY_URL, DEFAULT_TIMEOUT_MS};
pub use session_resolve::resolve_last_session;

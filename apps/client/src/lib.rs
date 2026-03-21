//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod connection;
mod config;
mod error;

pub use connection::AlephClient;
pub use config::{CliConfig, ManifestConfig};
pub use error::{CliError, CliResult};

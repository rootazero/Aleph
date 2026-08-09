//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod config;
mod connection;
mod error;
mod gateway_client;
pub mod output;
mod tls;

pub use config::{CliConfig, ManifestConfig};
pub use connection::AlephClient;
pub use error::{CliError, CliResult};
pub use gateway_client::{GatewayClient, DEFAULT_GATEWAY_URL, DEFAULT_TIMEOUT_MS};
pub use output::{
    print_error, print_json, print_list_table, print_success, print_table, OutputFormat,
};

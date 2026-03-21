//! Aleph Client Library
//!
//! Shared WebSocket JSON-RPC 2.0 client for communicating with Aleph Gateway.
//! Used by CLI, TUI, and any future client application.

mod connection;
mod config;
mod error;
mod gateway_client;
pub mod output;

pub use connection::AlephClient;
pub use config::{CliConfig, ManifestConfig};
pub use error::{CliError, CliResult};
pub use gateway_client::{GatewayClient, DEFAULT_GATEWAY_URL, DEFAULT_TIMEOUT_MS};
pub use output::{OutputFormat, print_json, print_table, print_list_table, print_success, print_error};

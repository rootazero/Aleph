//! CLI utilities for Aether Gateway commands.

pub mod channels;
pub mod client;
pub mod config;
pub mod error;
pub mod output;

pub use client::GatewayClient;
pub use error::CliError;
pub use output::{print_error, print_json, print_list_table, print_success, print_table, OutputFormat};

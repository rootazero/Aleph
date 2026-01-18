//! External MCP Server Support
//!
//! Provides runtime detection and connection management for external MCP servers.

mod connection;
mod runtime;

pub use connection::McpServerConnection;
pub use runtime::{check_runtime, RuntimeKind};

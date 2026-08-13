//! External MCP Server Support
//!
//! Provides runtime detection and connection management for external MCP servers.

mod connection;
mod runtime;

pub use connection::{ChangedLists, McpServerConnection};
// Re-exported so `browser::chrome_mcp` can ask whether an error is the tool's
// own verdict or a failure to reach it — a distinction `call_tool` flattens
// into `IoError`. See `connection::TOOL_ERROR_MARKER`.
pub(crate) use connection::is_tool_error;
pub use runtime::{check_runtime, RuntimeKind};

//! MCP Transport Layer
//!
//! Provides transport implementations for communicating with MCP servers.
//!
//! # Transport Trait
//!
//! The [`McpTransport`] trait defines the abstract interface for all MCP transports.
//! This allows for different transport mechanisms while maintaining a consistent API.
//!
//! # Available Transports
//!
//! - [`StdioTransport`] - Communicates with local MCP servers via subprocess stdio
//!
//! # Planned Transports
//!
//! - `HttpTransport` - Communicates with remote MCP servers via HTTP POST
//! - `SseTransport` - Communicates with remote MCP servers via HTTP + SSE

mod stdio;
mod traits;

pub use stdio::StdioTransport;
pub use traits::{McpTransport, NotificationCallback};

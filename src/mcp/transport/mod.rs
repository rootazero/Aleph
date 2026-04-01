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
//! - [`HttpTransport`] - Communicates with remote MCP servers via HTTP POST
//! - [`SseTransport`] - Communicates with remote MCP servers via HTTP POST + SSE notifications

mod http;
mod sse;
mod sse_events;
mod stdio;
mod traits;

pub use http::{HttpTransport, HttpTransportConfig};
pub use sse::{SseTransport, SseTransportConfig};
pub use sse_events::{SseEvent, SseNotification, SseRequest};
pub use stdio::StdioTransport;
pub use traits::{McpTransport, NotificationCallback};

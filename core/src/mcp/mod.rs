//! MCP (Model Context Protocol) Integration Module
//!
//! This module handles external MCP server connections.
//!
//! # Architecture
//!
//! - **MCP Tools**: Wrapped via `McpToolWrapper` in `rig_tools` module
//!   for integration with rig-core
//!
//! - **External MCP Servers**: Managed by `McpClient`
//!   - Connected via transport abstraction ([`McpTransport`] trait)
//!   - Tools discovered via JSON-RPC
//!
//! # Transport Layer
//!
//! The [`McpTransport`] trait provides an abstraction for different transport
//! mechanisms:
//!
//! - [`StdioTransport`] - Local servers via subprocess stdio
//! - `HttpTransport` - Remote servers via HTTP POST (planned)
//! - `SseTransport` - Remote servers via HTTP + SSE (planned)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Tool Sources                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Aether Tools              │  External MCP Servers              │
//! │  (see crate::rig_tools)    │  (see this mcp module)             │
//! │  ├── SearchTool            │  ├── McpTransport trait            │
//! │  ├── WebFetchTool          │  │   ├── StdioTransport            │
//! │  ├── YouTubeTool           │  │   ├── HttpTransport (planned)   │
//! │  └── McpToolWrapper        │  │   └── SseTransport (planned)    │
//! │                            │  └── Runtime Detection             │
//! │                            │      (node, python, bun)           │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

mod client;
pub mod external;
pub mod jsonrpc;
pub mod transport;
pub mod types;

pub use client::{ExternalServerConfig, McpClient, McpClientBuilder, McpStartupReport};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{
    IdGenerator, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
pub use transport::{McpTransport, NotificationCallback, StdioTransport};
pub use types::{
    McpEnvVar, McpResource, McpServerConfig, McpServerPermissions, McpServerStatus,
    McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig, McpTool, McpToolCall,
    McpToolInfo, McpToolResult,
};

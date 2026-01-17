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
//!   - Connected via stdio transport
//!   - Tools discovered via JSON-RPC
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Tool Sources                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Rig-Core Tools            │  External MCP Servers              │
//! │  (see crate::rig_tools)    │  (see this mcp module)             │
//! │  ├── SearchTool            │  ├── StdioTransport                │
//! │  ├── WebFetchTool          │  │   └── JSON-RPC over stdio       │
//! │  ├── YouTubeTool           │  └── Runtime Detection             │
//! │  └── McpToolWrapper        │      (node, python, bun)           │
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
pub use transport::StdioTransport;
pub use types::{
    McpEnvVar, McpResource, McpServerConfig, McpServerPermissions, McpServerStatus,
    McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig, McpTool, McpToolCall,
    McpToolInfo, McpToolResult,
};

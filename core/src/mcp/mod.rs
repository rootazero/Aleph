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
//! - **Resources**: Files, data, and content exposed by servers ([`McpResourceManager`])
//!
//! - **Prompts**: Reusable prompt templates from servers ([`McpPromptManager`])
//!
//! # Transport Layer
//!
//! The [`McpTransport`] trait provides an abstraction for different transport
//! mechanisms:
//!
//! - [`StdioTransport`] - Local servers via subprocess stdio
//! - [`HttpTransport`] - Remote servers via HTTP POST
//! - [`SseTransport`] - Remote servers via HTTP + SSE (notifications)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Tool Sources                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Aether Tools              │  External MCP Servers              │
//! │  (see crate::rig_tools)    │  (see this mcp module)             │
//! │  ├── SearchTool            │  ├── McpTransport trait            │
//! │  ├── WebFetchTool          │  │   ├── StdioTransport            │
//! │  ├── YouTubeTool           │  │   ├── HttpTransport             │
//! │  └── McpToolWrapper        │  │   └── SseTransport              │
//! │                            │  ├── Resources (McpResourceManager)│
//! │                            │  ├── Prompts (McpPromptManager)    │
//! │                            │  └── Runtime Detection             │
//! │                            │      (node, python, bun)           │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

mod client;
pub mod external;
pub mod jsonrpc;
mod notifications;
mod prompts;
mod resources;
pub mod transport;
pub mod types;

pub use client::{ExternalServerConfig, McpClient, McpClientBuilder, McpStartupReport};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{
    IdGenerator, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
pub use notifications::{McpEvent, McpEventHandler, McpNotificationRouter};
pub use prompts::{McpPrompt, McpPromptArgument, McpPromptManager, PromptContent, PromptMessage, PromptResult};
pub use resources::{McpResourceManager, ResourceContent};
pub use transport::{
    HttpTransport, HttpTransportConfig, McpTransport, NotificationCallback, SseTransport,
    SseTransportConfig, StdioTransport,
};
pub use types::{
    McpEnvVar, McpRemoteServerConfig, McpResource, McpServerConfig, McpServerPermissions,
    McpServerStatus, McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig,
    McpTool, McpToolCall, McpToolInfo, McpToolResult, TransportPreference,
};

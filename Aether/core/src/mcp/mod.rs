//! MCP (Model Context Protocol) Integration Module
//!
//! This module handles external MCP server connections.
//!
//! # Architecture
//!
//! - **Native Tools**: Implemented via `AgentTool` trait in `tools` module
//!   - fs, git, shell, system, clipboard, screen tools
//!   - Registered in `NativeToolRegistry` for execution
//!
//! - **External MCP Servers**: Managed by `McpClient`
//!   - Connected via stdio transport
//!   - Tools discovered via JSON-RPC
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Tool Sources                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Native Tools (AgentTool)    │  External MCP Servers            │
//! │  (see crate::tools module)   │  (see this mcp module)           │
//! │  ├── FileReadTool            │  ├── StdioTransport              │
//! │  ├── GitStatusTool           │  │   └── JSON-RPC over stdio     │
//! │  ├── ShellExecuteTool        │  └── Runtime Detection           │
//! │  └── ...                     │      (node, python, bun)         │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # MCP Tool Bridge
//!
//! The `bridge` module provides `McpToolBridge` which implements the `AgentTool`
//! trait for MCP tools. This allows seamless integration with the native function
//! calling infrastructure.
//!
//! ```rust,ignore
//! use aether_core::mcp::{McpClient, McpToolBridge};
//! use std::sync::Arc;
//!
//! let client = Arc::new(McpClient::new());
//! let bridges = McpToolBridge::from_client(client).await;
//! ```

pub mod bridge;
mod client;
pub mod external;
pub mod jsonrpc;
pub mod transport;
pub mod types;

pub use client::{ExternalServerConfig, McpClient, McpClientBuilder};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{IdGenerator, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use transport::StdioTransport;
pub use types::{
    McpEnvVar, McpResource, McpServerConfig, McpServerPermissions, McpServerStatus,
    McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig, McpTool, McpToolCall,
    McpToolInfo, McpToolResult,
};

// MCP Tool Bridge - implements AgentTool for MCP tools
pub use bridge::{create_bridges, McpToolBridge, McpToolSource};

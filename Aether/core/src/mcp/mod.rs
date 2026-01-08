//! MCP (Model Context Protocol) Integration Module
//!
//! This module provides MCP capability for Aether, including:
//! - Builtin services (fs, git, system_info, shell)
//! - External server management via stdio transport
//! - Tool routing and execution
//!
//! # Architecture
//!
//! The MCP module wraps the shared foundation services (`services::*`) with:
//! - MCP protocol adaptation (JSON-RPC style interface)
//! - Security controls (path validation, command whitelisting)
//! - Tool discovery and routing
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         McpClient                                │
//! │  (Service Registry + Tool Router)                               │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Builtin Services              │  External Servers              │
//! │  ├── FsService                 │  ├── StdioTransport            │
//! │  │   └── services::fs::LocalFs │  │   └── JSON-RPC over stdio   │
//! │  ├── GitService                │  └── Runtime Detection         │
//! │  │   └── services::git::GitRepo│      (node, python, bun)       │
//! │  ├── SystemInfoService         │                                │
//! │  │   └── services::system_info │                                │
//! │  └── ShellService              │                                │
//! │      └── (standalone impl)     │                                │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Zero External Dependency Guarantee
//!
//! All builtin services work without Node.js, Python, or git CLI:
//! - File operations: Pure Rust (tokio::fs)
//! - Git operations: git2-rs library
//! - System info: Rust std + platform APIs

pub mod builtin;
mod client;
pub mod external;
pub mod jsonrpc;
pub mod transport;
mod types;

pub use builtin::{
    BuiltinMcpService, FsService, GitService, ShellService, SystemInfoService,
};
pub use client::{ExternalServerConfig, McpClient, McpClientBuilder};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{IdGenerator, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use transport::StdioTransport;
pub use types::{
    McpResource, McpServiceInfo, McpSettingsConfig, McpTool, McpToolCall, McpToolInfo,
    McpToolResult,
};

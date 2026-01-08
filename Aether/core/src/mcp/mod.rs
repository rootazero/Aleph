//! MCP (Model Context Protocol) Integration Module
//!
//! This module handles external MCP servers (Tier 2 Extensions).
//! System tools (fs, git, shell, sys) are now in `services::tools`.
//!
//! # Two-Tier Tool Architecture
//!
//! - **Tier 1 (System Tools)**: Native Rust, always available, top-level commands
//!   - Located in `services::tools` module
//!   - Commands: `/fs`, `/git`, `/shell`, `/sys`
//!
//! - **Tier 2 (MCP Extensions)**: External processes, user-installed
//!   - Located in this `mcp` module
//!   - Commands: `/mcp/<server-name>`
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         McpClient                                │
//! │  (Service Registry + Tool Router)                               │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  System Tools (Tier 1)          │  MCP Extensions (Tier 2)      │
//! │  (see services::tools)          │  External Servers              │
//! │  ├── FsService     → /fs        │  ├── StdioTransport            │
//! │  ├── GitService    → /git       │  │   └── JSON-RPC over stdio   │
//! │  ├── SysService    → /sys       │  └── Runtime Detection         │
//! │  └── ShellService  → /shell     │      (node, python, bun)       │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

mod client;
pub mod external;
pub mod jsonrpc;
pub mod transport;
pub mod types;

// Re-export system tools from services::tools for backward compatibility
pub use crate::services::tools::{
    FsService, FsServiceConfig, GitService, GitServiceConfig, ShellService, ShellServiceConfig,
    SystemInfoService, SystemTool,
};
// Backward compatibility alias
pub use crate::services::tools::BuiltinMcpService;

pub use client::{ExternalServerConfig, McpClient, McpClientBuilder};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{IdGenerator, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use transport::StdioTransport;
pub use types::{
    McpEnvVar, McpResource, McpServerConfig, McpServerPermissions, McpServerStatus,
    McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig, McpTool, McpToolCall,
    McpToolInfo, McpToolResult,
};

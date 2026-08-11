//! MCP (Model Context Protocol) Integration Module
//!
//! This module handles external MCP server connections.
//!
//! # Architecture
//!
//! - **MCP Tools**: Wrapped via `McpToolWrapper` in `builtin_tools` module
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
//! - [`HttpTransport`] - Remote servers via HTTP POST
//! - [`SseTransport`] - Remote servers via HTTP + SSE (notifications)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Tool Sources                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Aleph Tools              │  External MCP Servers              │
//! │  (see crate::builtin_tools)    │  (see this mcp module)             │
//! │  ├── SearchTool            │  ├── McpTransport trait            │
//! │  ├── WebFetchTool          │  │   ├── StdioTransport            │
//! │  └── McpToolWrapper        │  │   ├── HttpTransport             │
//! │                            │  │   └── SseTransport              │
//! │                            │  ├── Resources                     │
//! │                            │  ├── Prompts                       │
//! │                            │  └── Runtime Detection             │
//! │                            │      (node, python, bun)           │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

pub mod auth;
mod client;
mod context_injector;
pub mod error_class;
pub mod external;
pub mod jsonrpc;
pub mod manager;
pub mod modern;
mod preflight;
pub mod presets;
mod prompts;
pub mod protocol;
mod redact;
mod resources;
pub mod sampling;
pub mod sampling_bridge;
pub(crate) mod tool_bridge;
mod tool_sanitize;
pub mod transport;
pub mod types;

pub use auth::{
    CallbackResult, CallbackServer, ClientInfo, OAuthEntry, OAuthProvider, OAuthServerMetadata,
    OAuthStorage, OAuthTokens,
};
pub use client::{ExternalServerConfig, McpClient};
pub use context_injector::{ContextInjector, InjectedContext, ResourceContext, ToolContext};
pub use error_class::{classify_mcp_error, McpErrorKind};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{
    IdGenerator, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
pub use preflight::preflight_remote_url;
pub use prompts::{McpPrompt, McpPromptArgument, PromptContent, PromptMessage, PromptResult};
pub use protocol::IncludeContext;
pub use redact::redact_mcp_error;
pub use resources::ResourceContent;
pub use sampling::{SamplingCallback, SamplingHandler};
pub use sampling_bridge::{register_sampling_llm, sampling_llm_registered, serve_sampling};
pub use tool_bridge::spawn_tool_bridge;
#[cfg(test)]
pub(crate) use tool_bridge::CAPABILITY_READ_BUILTIN_NAMES;
pub use tool_sanitize::{normalize_tool_schema, scan_description_for_injection};
pub use transport::{
    HttpTransport, HttpTransportConfig, McpTransport, NotificationCallback, SseTransport,
    SseTransportConfig, StdioTransport,
};
pub use types::{
    McpRemoteServerConfig, McpResource, McpTool, McpToolFilter, McpToolResult, TransportPreference,
};

// Manager types (MCP orchestration layer)
pub use manager::{
    HealthCheckConfig, HealthStatus, McpCommand, McpManagerActor, McpManagerConfig,
    McpManagerEvent, McpManagerHandle, McpPersistentConfig, McpServerInfo, McpServerStatusDetail,
    McpTransportType, ServerHealth,
};
pub use presets::{McpPreset, PresetCategory, PresetEnvVar, PresetTransport};

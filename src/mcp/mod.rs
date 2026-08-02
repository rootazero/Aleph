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
//! │  Aleph Tools              │  External MCP Servers              │
//! │  (see crate::builtin_tools)    │  (see this mcp module)             │
//! │  ├── SearchTool            │  ├── McpTransport trait            │
//! │  ├── WebFetchTool          │  │   ├── StdioTransport            │
//! │  └── McpToolWrapper        │  │   ├── HttpTransport             │
//! │                            │  │   └── SseTransport              │
//! │                            │  ├── Resources (McpResourceManager)│
//! │                            │  ├── Prompts (McpPromptManager)    │
//! │                            │  └── Runtime Detection             │
//! │                            │      (node, python, bun)           │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

mod approval;
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
mod tool_bridge;
mod tool_sanitize;
pub mod transport;
pub mod types;

pub use approval::{ApprovalHandler, ApprovalPresentCallback};
pub use auth::{
    AuthorizationRequest, CallbackResult, CallbackServer, ClientInfo, OAuthEntry, OAuthProvider,
    OAuthServerMetadata, OAuthStorage, OAuthTokens, DEFAULT_CALLBACK_PORT,
};
pub use client::{ExternalServerConfig, McpClient, McpClientBuilder, McpStartupReport};
pub use context_injector::{ContextInjector, InjectedContext, ResourceContext, ToolContext};
pub use error_class::{classify_mcp_error, McpErrorKind};
pub use external::{check_runtime, McpServerConnection, RuntimeKind};
pub use jsonrpc::{
    IdGenerator, JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
};
pub use preflight::preflight_remote_url;
pub use prompts::{
    McpPrompt, McpPromptArgument, McpPromptManager, PromptContent, PromptMessage, PromptResult,
};
pub use protocol::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, IncludeContext, SamplingChunk,
};
pub use redact::redact_mcp_error;
pub use resources::{McpResourceManager, ResourceContent};
pub use sampling::{
    extract_system_prompt, sampling_messages_to_chat, SamplingCallback, SamplingHandler,
};
pub use tool_bridge::spawn_tool_bridge;
#[cfg(test)]
pub(crate) use tool_bridge::CAPABILITY_READ_BUILTIN_NAMES;
pub use tool_sanitize::{normalize_tool_schema, scan_description_for_injection};
pub use transport::{
    HttpTransport, HttpTransportConfig, McpTransport, NotificationCallback, SseTransport,
    SseTransportConfig, StdioTransport,
};
pub use types::{
    McpEnvVar, McpRemoteServerConfig, McpResource, McpServerConfig, McpServerPermissions,
    McpServerStatus, McpServerStatusInfo, McpServerType, McpServiceInfo, McpSettingsConfig,
    McpTool, McpToolCall, McpToolFilter, McpToolInfo, McpToolResult, TransportPreference,
};

// Manager types (MCP orchestration layer)
pub use manager::{
    HealthCheckConfig, HealthStatus, McpCommand, McpManagerActor, McpManagerConfig,
    McpManagerEvent, McpManagerHandle, McpPersistentConfig, McpServerInfo, McpServerStatusDetail,
    McpTransportType, ServerHealth,
};
pub use presets::{McpPreset, PresetCategory, PresetEnvVar, PresetTransport, Reachability};

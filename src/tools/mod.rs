//! Aleph Tool System
//!
//! Self-implemented tool traits replacing rig-core dependency.
//!
//! This module provides:
//! - `AlephTool`: Static dispatch trait for compile-time known tools
//! - `AlephToolDyn`: Dynamic dispatch trait for runtime-loaded tools (MCP, plugins)
//! - `AlephToolServer`: Tool server with hot-reload support
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    AlephTool (static)                       │
//! │   Compile-time known tools with typed Args/Output           │
//! │   Auto JSON Schema generation via schemars                   │
//! └─────────────────────────────────┬───────────────────────────┘
//!                                   │ Blanket impl
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   AlephToolDyn (dynamic)                    │
//! │   Runtime dispatch with JSON Value args                      │
//! │   Used by: MCP tools, plugin tools, hot-reloaded tools      │
//! └─────────────────────────────────┬───────────────────────────┘
//!                                   │
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   AlephToolServer                           │
//! │   Hot-reload enabled tool registry                           │
//! │   Thread-safe add/remove/list/call operations               │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod adapters;
mod builtin;
pub mod context;
pub mod execution_context;
pub mod info;
pub mod orchestrator;
pub mod pipeline;
pub mod refresh;
pub mod result_store;
pub mod runtime;
mod server;
mod traits;
mod types;

// Markdown skill system
pub mod markdown_skill;

// Schema strictification for strict-mode tool calling
pub mod schema_strictify;

// Consumer-side `ToolService` trait + production `ScopedToolService` adapter.
// The pre-`ScopedToolService` decorator chain (Phase 2 facade — `facade.rs`,
// `dispatch.rs`, `middleware/`) has been deleted: gateway always supplies a
// per-request `ScopedToolService` via
// `tool_service_builder::build_request_tool_service`, leaving the chain
// unreachable. `NullToolService` is the fail-closed default that fills the
// harness fallback slot.
//
// `tools::registry::ToolRegistry` + `tools::handlers::*` survive — they are
// the live target of `mcp::tool_bridge`, which mutates the registry as MCP
// servers advertise / drop tools.
pub mod handlers;
pub mod mcp_scope_view;
pub mod null;
pub mod registry;
pub mod scoped;
pub mod service;
pub mod turn_context;
pub use scoped::ScopedToolService;

pub use context::{new_tool_context_handle, ToolContext, ToolContextHandle};
pub use null::NullToolService;
pub use registry::ToolRegistry;
pub use server::{AlephToolServer, AlephToolServerHandle};
pub use service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource};
pub use traits::{AlephTool, AlephToolDyn};
pub use types::{ToolRepairInfo, ToolRepairType, ToolUpdateInfo};

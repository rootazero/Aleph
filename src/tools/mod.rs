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
pub mod attempt_summary;
pub mod budget;
pub mod concurrency;
pub mod context;
pub mod error_kind;
pub mod fallback_registry;
pub mod fs_scope;
pub mod gather_budget;
pub mod in_flight;
pub mod info;
pub mod name_repair;
pub mod no_progress;
pub mod path_locks;
pub mod plan_gate;
pub mod redundant_calls;
pub mod result_processing;
pub mod result_store;
pub mod retry;
pub mod runtime;
mod server;
mod traits;
pub mod turn_budget;
mod types;
pub mod usage;

// Markdown skill system
pub mod markdown_skill;

pub mod schema_lookup;
pub mod tool_search;

// Schema strictification for strict-mode tool calling
pub mod schema_strictify;

// Plain-text tool-call promotion (openclaw tool-call-repair parity): recover
// tool calls that weaker models emit as assistant text instead of native
// function-call blocks.
pub mod text_tool_call;

// Consumer-side `ToolService` trait + production `ScopedToolService` adapter.
// The pre-`ScopedToolService` decorator chain (Phase 2 facade — `facade.rs`,
// `dispatch.rs`, `middleware/`) has been deleted: gateway always supplies a
// per-request `ScopedToolService` via
// `tool_service_builder::build_request_tool_service`, leaving the chain
// unreachable. `NullToolService` is the fail-closed default that fills the
// harness fallback slot.
//
// `tools::registry::ToolHandlerRegistry` + `tools::handlers::*` survive — they are
// the live target of `mcp::tool_bridge`, which mutates the registry as MCP
// servers advertise / drop tools.
pub mod handlers;
pub mod mcp_scope_view;
pub mod null;
pub mod probes;
pub mod registry;
pub mod runtime_state;
pub mod scoped;
pub mod service;
pub mod turn_context;
pub use scoped::{ScopedToolService, ToolDefinitionRewriter};

pub use context::{new_tool_context_handle, ToolContext, ToolContextHandle};
pub use null::NullToolService;
pub use registry::ToolHandlerRegistry;
pub use server::AlephToolServer;
pub use service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource};
pub use traits::{AlephTool, AlephToolDyn};
pub use types::ToolUpdateInfo;

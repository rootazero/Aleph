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

// Phase 2 Tool Service façade — consumer-side trait + decorator chain scaffold.
pub mod dispatch;
pub mod facade;
pub mod handlers;
pub mod middleware;
pub mod registry;
pub mod scoped;
pub mod service;
#[cfg(feature = "phase7_traffic_flip")]
pub use scoped::ScopedToolService;

pub use context::{new_tool_context_handle, ToolContext, ToolContextHandle};
pub use facade::{build_tool_service, build_tool_service_with_handles, ToolServiceHandles};
pub use registry::ToolRegistry;
pub use server::{AlephToolServer, AlephToolServerHandle};
pub use service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource};
pub use traits::{AlephTool, AlephToolDyn};
pub use types::{ToolRepairInfo, ToolRepairType, ToolUpdateInfo};

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

mod builtin;
mod server;
pub mod sessions;
mod traits;
mod types;

// Markdown skill system
pub mod markdown_skill;

pub use server::{AlephToolServer, AlephToolServerHandle};
pub use traits::{AlephTool, AlephToolDyn};
pub use types::{ToolRepairInfo, ToolRepairType, ToolUpdateInfo};

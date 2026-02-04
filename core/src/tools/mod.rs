//! Aether Tool System
//!
//! Self-implemented tool traits replacing rig-core dependency.
//!
//! This module provides:
//! - `AetherTool`: Static dispatch trait for compile-time known tools
//! - `AetherToolDyn`: Dynamic dispatch trait for runtime-loaded tools (MCP, plugins)
//! - `AetherToolServer`: Tool server with hot-reload support
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    AetherTool (static)                       │
//! │   Compile-time known tools with typed Args/Output           │
//! │   Auto JSON Schema generation via schemars                   │
//! └─────────────────────────────────┬───────────────────────────┘
//!                                   │ Blanket impl
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   AetherToolDyn (dynamic)                    │
//! │   Runtime dispatch with JSON Value args                      │
//! │   Used by: MCP tools, plugin tools, hot-reloaded tools      │
//! └─────────────────────────────────┬───────────────────────────┘
//!                                   │
//!                                   ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   AetherToolServer                           │
//! │   Hot-reload enabled tool registry                           │
//! │   Thread-safe add/remove/list/call operations               │
//! └─────────────────────────────────────────────────────────────┘
//! ```

mod server;
pub mod sessions;
mod traits;

// Markdown skill system
pub mod markdown_skill;

pub use server::{AetherToolServer, AetherToolServerHandle};
pub use traits::{AetherTool, AetherToolDyn};

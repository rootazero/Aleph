//! Plugin Runtime Systems
//!
//! Provides execution environments for plugins:
//! - WASM (Extism) - Sandboxed WebAssembly execution
//! - Static - Markdown-based skills/commands/agents (no runtime needed)
//! - MCP - External process via MCP protocol (managed by McpManager)

pub mod wasm;

// Re-export WASM runtime types
pub use wasm::{PermissionChecker, WasmRuntime, WasmToolInput, WasmToolOutput};

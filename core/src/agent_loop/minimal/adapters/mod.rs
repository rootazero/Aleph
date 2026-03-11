//! Adapters bridging existing tool traits to MinimalTool.

mod builtin_adapter;
pub mod mcp_adapter;
pub mod memory_adapter;

pub use builtin_adapter::BuiltinToolAdapter;
pub use mcp_adapter::{McpToolAdapter, McpToolSpec, McpTransportTrait};
pub use memory_adapter::{MemoryBackend, MemoryEntry, MemorySearchTool, MemoryStoreTool};

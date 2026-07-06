//! Adapters bridging existing tool traits to `LoopTool`.

pub mod mcp_adapter;
pub mod memory_adapter;
pub mod registry_adapter;

pub use mcp_adapter::McpRegistryTool;
pub use memory_adapter::{MemoryBackend, MemoryEntry, MemorySearchTool, MemoryStoreTool};
pub use registry_adapter::{build_registry_from_tools, build_tool_adapters_from_tools};

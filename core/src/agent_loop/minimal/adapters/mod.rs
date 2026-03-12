//! Adapters bridging existing tool traits to MinimalTool.

mod builtin_adapter;
pub mod daemon_adapter;
pub mod mcp_adapter;
pub mod memory_adapter;
pub mod registry_adapter;

pub use builtin_adapter::BuiltinToolAdapter;
pub use daemon_adapter::{DaemonBackend, DaemonEvent, DaemonQueryTool, DaemonSubscribeTool};
pub use mcp_adapter::{McpToolAdapter, McpToolSpec, McpTransportTrait};
pub use memory_adapter::{MemoryBackend, MemoryEntry, MemorySearchTool, MemoryStoreTool};
pub use registry_adapter::build_registry_from_tools;

//! Capability strategy implementations.
//!
//! This module contains the concrete implementations of capability strategies:
//! - `MemoryStrategy`: Vector-based memory retrieval
//! - `SearchStrategy`: Web search execution
//! - `McpStrategy`: MCP (Model Context Protocol) execution (placeholder)
//! - `VideoStrategy`: YouTube transcript extraction
//! - `SkillsStrategy`: Claude Agent Skills instruction injection

pub mod memory;
pub mod mcp;
pub mod search;
pub mod skills;
pub mod video;

// Re-exports for convenience
pub use memory::MemoryStrategy;
pub use mcp::McpStrategy;
pub use search::SearchStrategy;
pub use skills::SkillsStrategy;
pub use video::VideoStrategy;

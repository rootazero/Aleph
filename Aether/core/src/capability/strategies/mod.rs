//! Capability strategy implementations.
//!
//! This module contains the concrete implementations of capability strategies:
//! - `MemoryStrategy`: Vector-based memory retrieval (RAG context enrichment)
//! - `McpStrategy`: MCP (Model Context Protocol) tool access
//! - `SkillsStrategy`: Claude Agent Skills instruction injection

pub mod memory;
pub mod mcp;
pub mod skills;

// Re-exports for convenience
pub use memory::MemoryStrategy;
pub use mcp::McpStrategy;
pub use skills::SkillsStrategy;

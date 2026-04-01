//! Capability strategy implementations.
//!
//! This module contains the concrete implementations of capability strategies:
//! - `MemoryStrategy`: Vector-based memory retrieval (RAG context enrichment)
//! - `McpStrategy`: MCP (Model Context Protocol) tool access
//! - `SkillsStrategy`: Claude Agent Skills instruction injection

pub mod mcp;
pub mod memory;
pub mod skills;

// Re-exports for convenience
pub use mcp::McpStrategy;
pub use memory::MemoryStrategy;
pub use skills::SkillsStrategy;

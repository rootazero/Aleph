//! Plugin component loaders
//!
//! Parsers for individual plugin components (skills, hooks, agents, MCP).

mod agent;
pub mod hook;
mod mcp;
pub mod skill;

pub use agent::AgentLoader;
pub use hook::HookLoader;
pub use mcp::McpLoader;
pub use skill::SkillLoader;

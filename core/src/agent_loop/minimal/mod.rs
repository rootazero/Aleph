//! Minimal agent loop — unified tool trait and flat registry.
//!
//! Replaces the 3-trait tool hierarchy (AlephTool + AlephToolDyn + CapabilityStrategy)
//! with a single `MinimalTool` trait and a flat `MinimalToolRegistry`.

mod prompt_builder;
mod safety;
mod tool;

pub use prompt_builder::{MinimalPromptBuilder, ToolInfo};
pub use safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
pub use tool::{MinimalTool, MinimalToolRegistry, ToolDefinition, ToolResult};

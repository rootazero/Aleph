//! Minimal agent loop — unified tool trait and flat registry.
//!
//! Replaces the 3-trait tool hierarchy (AlephTool + AlephToolDyn + CapabilityStrategy)
//! with a single `MinimalTool` trait and a flat `MinimalToolRegistry`.

mod tool;

pub use tool::{MinimalTool, MinimalToolRegistry, ToolDefinition, ToolResult};

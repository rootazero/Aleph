//! Tool Type Definitions
//!
//! This module provides core type definitions for tool metadata.
//!
//! **Note**: The legacy `AgentTool` trait and `NativeToolRegistry` have been removed.
//! Tools now use rig-core's `Tool` trait for AI agent integration.
//! See `src/rig_tools/` for the new tool implementations.

mod traits;

pub use traits::{ToolCategory, ToolDefinition, ToolResult};

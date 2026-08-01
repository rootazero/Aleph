//! Tool Metadata Type Definitions
//!
//! Core data structures for the tool metadata layer.
//!
//! This module contains all tool-related type definitions organized into submodules:
//!
//! ## Submodules
//!
//! - `category`: Tool category for UI grouping and filtering
//! - `definition`: Tool definition and structured metadata types
//! - `result`: Tool execution result
//! - `safety`: Tool safety level classification
//! - `conflict`: Conflict resolution system for flat namespace
//! - `unified`: Unified tool representation
//! - `index`: Tool index system for smart discovery
//! - `tool_info`: Simplified types for Gateway JSON-RPC

mod category;
mod conflict;
mod definition;
mod safety;
mod tool_info;
mod unified;

// =============================================================================
// Re-exports
// =============================================================================

// Tool Category
pub use category::ToolCategory;

// Tool Definition
pub use definition::ToolDefinition;

// Tool Safety Level
pub use safety::ToolSafetyLevel;

// Conflict Resolution System
pub use conflict::{ConflictInfo, ConflictResolution, ToolPriority, ToolSource};

// Unified Tool
pub use unified::UnifiedTool;

// Dispatch & Channel Types
pub use unified::{ChannelType, DispatchMode};

// Tool Info Types (for Gateway JSON-RPC)
pub use tool_info::ToolSourceType;

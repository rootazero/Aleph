//! Dispatcher Type Definitions
//!
//! Core data structures for the Dispatcher Layer.
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
//! - `routing`: Routing layer indicator
//! - `index`: Tool index system for smart discovery
//! - `tool_info`: Simplified types for Gateway JSON-RPC

mod category;
mod conflict;
mod definition;
mod execution_policy;
mod tool_info;
mod index;
mod result;
mod routing;
mod safety;
mod unified;

// =============================================================================
// Re-exports
// =============================================================================

// Tool Category
pub use category::ToolCategory;

// Tool Definition and Structured Types
pub use definition::{StructuredToolMeta, ToolDefinition, ToolDiff};

// Tool Result
pub use result::ToolResult;

// Tool Safety Level
pub use safety::ToolSafetyLevel;

// Conflict Resolution System
pub use conflict::{ConflictInfo, ConflictResolution, ToolPriority, ToolSource};

// Unified Tool
pub use unified::UnifiedTool;

// Routing Layer
pub use routing::RoutingLayer;

// Tool Index System
pub use index::{ToolIndex, ToolIndexCategory, ToolIndexEntry};

// Tool Info Types (for Gateway JSON-RPC)
pub use tool_info::{ToolSourceType, UnifiedToolInfo};

// Execution Policy (for Server-Client routing)
pub use execution_policy::ExecutionPolicy;

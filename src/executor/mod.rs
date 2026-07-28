//! Executor Module
//!
//! This module provides task execution capabilities for the Agent Loop architecture.
//!
//! # Types
//!
//! - [`ExecutionResult`]: The outcome of executing a task
//! - [`ToolCallRecord`]: Record of a tool call during execution
//! - [`TaskExecutionResult`]: Result of executing a single task
//! - [`ExecutionContext`]: Context information for execution
//! - [`ExecutorError`]: Error types for executor operations
//! - [`ToolRegistry`]: Tool lookup + execution trait for the agent loop
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::executor::{ExecutionResult, ExecutionContext, ExecutorError};
//!
//! // Create results for testing:
//! let result = ExecutionResult::success("Task completed successfully")
//!     .with_execution_time_ms(150);
//!
//! // Create a failed result
//! let result = ExecutionResult::failure("Connection timeout");
//! ```

mod builtin_registry;
mod tool_registry;
mod types;

pub use builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolConfig, BuiltinToolRegistry,
    BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES,
};
pub use tool_registry::ToolRegistry;
pub use types::{
    ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult, ToolCallRecord,
};

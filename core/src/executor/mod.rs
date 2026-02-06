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
//! - [`SingleStepExecutor`]: Single-step task executor
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::executor::{ExecutionResult, ExecutionContext, ExecutorError};
//! use alephcore::executor::{SingleStepExecutor, SingleStepConfig};
//!
//! // Create executor
//! let executor = SingleStepExecutor::new(config);
//!
//! // Create results for testing:
//! let result = ExecutionResult::success("Task completed successfully")
//!     .with_execution_time_ms(150);
//!
//! // Create a failed result
//! let result = ExecutionResult::failure("Connection timeout");
//! ```

mod builtin_registry;
mod cache_config;
mod cache_store;
mod router;
mod single_step;
mod types;

pub use builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, is_builtin_tool, BuiltinToolConfig,
    BuiltinToolDefinition, BuiltinToolRegistry, BUILTIN_TOOL_DEFINITIONS,
};
pub use cache_config::ToolCacheConfig;
pub use cache_store::{CacheStats, ToolResultCache};
pub use router::{RoutingDecision, ToolRouter};
pub use single_step::{SingleStepConfig, SingleStepExecutor, ToolRegistry};
pub use types::{
    ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult, ToolCallRecord,
};

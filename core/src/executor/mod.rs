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

pub mod action_types;
mod builtin_registry;
#[allow(dead_code)]
mod cache_config;
#[allow(dead_code)]
mod cache_store;
mod exec_security_gate;
#[allow(dead_code)]
mod single_step;
#[allow(dead_code)]
mod types;

pub use builtin_registry::{
    create_tool_boxed, get_builtin_tool_names, BuiltinToolConfig, BuiltinToolRegistry,
    BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES,
};
pub use exec_security_gate::ExecSecurityGate;
pub(crate) use exec_security_gate::PreExecDecision;
pub use single_step::ToolRegistry;
pub use types::{ExecutionContext, ExecutionResult, ToolCallRecord};

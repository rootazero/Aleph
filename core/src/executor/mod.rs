//! Unified Executor Module
//!
//! This module implements the unified executor architecture that executes
//! plans produced by the unified planner.
//!
//! # Architecture
//!
//! The unified executor takes an `ExecutionPlan` and executes it, producing
//! an `ExecutionResult`. It handles three types of plans:
//!
//! - **Conversational**: Direct AI response, no tools needed
//! - **SingleAction**: Execute a single tool call
//! - **TaskGraph**: Execute a multi-step task graph with dependencies
//!
//! # Types
//!
//! - [`ExecutionResult`]: The outcome of executing a plan
//! - [`ToolCallRecord`]: Record of a tool call during execution
//! - [`TaskExecutionResult`]: Result of executing a single task
//! - [`ExecutionContext`]: Context information for execution
//! - [`ExecutorError`]: Error types for executor operations
//! - [`UnifiedExecutor`]: The main executor implementation
//! - [`ExecutorConfig`]: Configuration for the executor
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::executor::{ExecutionResult, ExecutionContext, ExecutorError};
//! use aethecore::executor::{UnifiedExecutor, ExecutorConfig};
//!
//! // Create executor with default config
//! let executor = UnifiedExecutor::new(agent_manager, executor_registry, event_handler);
//!
//! // Execute a plan and get the result
//! let result = executor.execute(plan, context).await?;
//!
//! // Check the result
//! if result.success {
//!     println!("Execution completed: {}", result.content);
//!     println!("Tool calls made: {}", result.tool_calls.len());
//! } else {
//!     eprintln!("Execution failed: {:?}", result.error);
//! }
//!
//! // Or create results manually for testing:
//!
//! // Create a successful result
//! let result = ExecutionResult::success("Task completed successfully")
//!     .with_execution_time_ms(150);
//!
//! // Create a failed result
//! let result = ExecutionResult::failure("Connection timeout");
//! ```

mod single_step;
mod types;
mod unified;

pub use single_step::{SingleStepConfig, SingleStepExecutor, ToolRegistry};
pub use types::{
    ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult, ToolCallRecord,
};
pub use unified::{ExecutorConfig, UnifiedExecutor};

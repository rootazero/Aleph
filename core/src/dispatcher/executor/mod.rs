//! Executor module
//!
//! This module provides the executor registry and trait for task execution.

mod code_exec;
mod file_ops;
mod noop;
mod permission;
mod registry;

pub use code_exec::{CodeExecError, CodeExecResult, CodeExecutor, CommandChecker, RuntimeInfo};
pub use file_ops::FileOpsExecutor;
pub use noop::NoopExecutor;
pub use permission::{FileOpError, PathPermissionChecker};
pub use registry::ExecutorRegistry;

use async_trait::async_trait;

use crate::dispatcher::agent_types::{Task, TaskResult, TaskType};
use crate::error::Result;

/// Context provided to executors during task execution
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// ID of the task graph being executed
    pub graph_id: String,

    /// Whether the execution is in dry-run mode
    pub dry_run: bool,

    /// Working directory for file operations
    pub working_dir: Option<String>,

    /// Additional context data
    pub extra: serde_json::Value,
}

impl ExecutionContext {
    /// Create a new execution context
    pub fn new(graph_id: impl Into<String>) -> Self {
        Self {
            graph_id: graph_id.into(),
            dry_run: false,
            working_dir: None,
            extra: serde_json::Value::Null,
        }
    }

    /// Set dry-run mode
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }
}

/// Trait for task executors
///
/// Executors handle the actual execution of specific task types.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Get the task types this executor can handle
    fn supported_types(&self) -> Vec<&'static str>;

    /// Check if this executor can handle a specific task type
    fn can_execute(&self, task_type: &TaskType) -> bool;

    /// Execute a task
    ///
    /// # Arguments
    ///
    /// * `task` - The task to execute
    /// * `ctx` - Execution context
    ///
    /// # Returns
    ///
    /// * `Ok(TaskResult)` - If execution succeeds
    /// * `Err` - If execution fails
    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult>;

    /// Cancel an executing task
    ///
    /// # Arguments
    ///
    /// * `task_id` - ID of the task to cancel
    ///
    /// Note: Not all executors support cancellation. Default implementation does nothing.
    async fn cancel(&self, _task_id: &str) -> Result<()> {
        Ok(())
    }

    /// Get the name of this executor
    fn name(&self) -> &str;
}

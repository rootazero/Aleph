//! Executor registry implementation

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

use super::{ExecutionContext, TaskExecutor};
use crate::cowork::types::{Task, TaskResult, TaskType};
use crate::error::{AetherError, Result};

/// Registry for task executors
///
/// Manages a collection of executors and routes tasks to the appropriate one.
pub struct ExecutorRegistry {
    executors: HashMap<String, Arc<dyn TaskExecutor>>,
}

impl ExecutorRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            executors: HashMap::new(),
        }
    }

    /// Register an executor
    ///
    /// If an executor with the same name already exists, it will be replaced.
    pub fn register(&mut self, name: impl Into<String>, executor: Arc<dyn TaskExecutor>) {
        let name = name.into();
        debug!("Registering executor: {}", name);
        self.executors.insert(name, executor);
    }

    /// Unregister an executor
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn TaskExecutor>> {
        self.executors.remove(name)
    }

    /// Get an executor by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn TaskExecutor>> {
        self.executors.get(name)
    }

    /// Find an executor that can handle the given task type
    pub fn find_executor(&self, task_type: &TaskType) -> Option<&Arc<dyn TaskExecutor>> {
        self.executors
            .values()
            .find(|e| e.can_execute(task_type))
    }

    /// Execute a task using the appropriate executor
    pub async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult> {
        let executor = self.find_executor(&task.task_type).ok_or_else(|| {
            warn!("No executor found for task type: {:?}", task.task_type);
            AetherError::Other {
                message: format!("No executor found for task type: {}", task.task_type.category()),
                suggestion: Some("Register an executor for this task type".to_string()),
            }
        })?;

        debug!(
            "Executing task '{}' with executor '{}'",
            task.id,
            executor.name()
        );

        executor.execute(task, ctx).await
    }

    /// Cancel a task
    pub async fn cancel(&self, task: &Task) -> Result<()> {
        if let Some(executor) = self.find_executor(&task.task_type) {
            executor.cancel(&task.id).await
        } else {
            Ok(()) // No executor means nothing to cancel
        }
    }

    /// Get the number of registered executors
    pub fn len(&self) -> usize {
        self.executors.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }

    /// List all registered executor names
    pub fn executor_names(&self) -> Vec<&str> {
        self.executors.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::executor::NoopExecutor;
    use crate::cowork::types::{FileOp, TaskType};
    use std::path::PathBuf;

    #[test]
    fn test_registry_register() {
        let mut registry = ExecutorRegistry::new();
        assert!(registry.is_empty());

        let executor = Arc::new(NoopExecutor::new());
        registry.register("noop", executor);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("noop").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_find_executor() {
        let mut registry = ExecutorRegistry::new();
        registry.register("noop", Arc::new(NoopExecutor::new()));

        let task_type = TaskType::FileOperation(FileOp::List {
            path: PathBuf::from("/tmp"),
        });

        let executor = registry.find_executor(&task_type);
        assert!(executor.is_some());
    }

    #[tokio::test]
    async fn test_registry_execute() {
        let mut registry = ExecutorRegistry::new();
        registry.register("noop", Arc::new(NoopExecutor::new()));

        let task = Task::new(
            "test_1",
            "Test Task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let ctx = ExecutionContext::new("graph_1");
        let result = registry.execute(&task, &ctx).await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_registry_unregister() {
        let mut registry = ExecutorRegistry::new();
        registry.register("noop", Arc::new(NoopExecutor::new()));

        assert_eq!(registry.len(), 1);

        let removed = registry.unregister("noop");
        assert!(removed.is_some());
        assert!(registry.is_empty());
    }
}

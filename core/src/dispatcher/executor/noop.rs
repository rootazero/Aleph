//! No-operation executor for testing

use async_trait::async_trait;
use std::time::Duration;
use tracing::debug;

use super::{ExecutionContext, TaskExecutor};
use crate::dispatcher::cowork_types::{Task, TaskResult, TaskType};
use crate::error::Result;

/// A no-operation executor that returns mock results
///
/// Useful for testing the task orchestration system without
/// actually performing any operations.
pub struct NoopExecutor {
    /// Simulated execution delay
    delay: Duration,
}

impl NoopExecutor {
    /// Create a new NoopExecutor with no delay
    pub fn new() -> Self {
        Self {
            delay: Duration::ZERO,
        }
    }

    /// Create a NoopExecutor with a simulated delay
    pub fn with_delay(delay: Duration) -> Self {
        Self { delay }
    }
}

impl Default for NoopExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskExecutor for NoopExecutor {
    fn supported_types(&self) -> Vec<&'static str> {
        vec![
            "file_operation",
            "code_execution",
            "document_generation",
            "app_automation",
            "ai_inference",
        ]
    }

    fn can_execute(&self, _task_type: &TaskType) -> bool {
        // NoopExecutor can "handle" all task types
        true
    }

    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult> {
        debug!("NoopExecutor executing task: {} ({})", task.name, task.id);

        if ctx.dry_run {
            debug!("Dry run mode - skipping execution");
        } else if !self.delay.is_zero() {
            debug!("Simulating execution delay: {:?}", self.delay);
            tokio::time::sleep(self.delay).await;
        }

        Ok(
            TaskResult::with_string(format!("NoopExecutor completed task: {}", task.name))
                .with_duration(self.delay)
                .with_summary(format!(
                    "Mock execution of {} task '{}'",
                    task.task_type.category(),
                    task.name
                )),
        )
    }

    fn name(&self) -> &str {
        "NoopExecutor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::cowork_types::{FileOp, TaskType};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_noop_executor() {
        let executor = NoopExecutor::new();

        let task = Task::new(
            "test_1",
            "Test Task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let ctx = ExecutionContext::new("graph_1");
        let result = executor.execute(&task, &ctx).await.unwrap();

        assert!(result.summary.is_some());
        assert!(result.summary.unwrap().contains("Test Task"));
    }

    #[tokio::test]
    async fn test_noop_executor_with_delay() {
        let executor = NoopExecutor::with_delay(Duration::from_millis(100));

        let task = Task::new(
            "test_1",
            "Delayed Task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let ctx = ExecutionContext::new("graph_1");
        let start = std::time::Instant::now();
        let result = executor.execute(&task, &ctx).await.unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(100));
        assert_eq!(result.duration, Duration::from_millis(100));
    }

    #[test]
    fn test_noop_executor_can_execute() {
        let executor = NoopExecutor::new();

        assert!(executor.can_execute(&TaskType::FileOperation(FileOp::List {
            path: PathBuf::from("/tmp")
        })));

        assert!(executor.can_execute(&TaskType::AiInference(
            crate::dispatcher::cowork_types::AiTask {
                prompt: "test".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }
        )));
    }
}

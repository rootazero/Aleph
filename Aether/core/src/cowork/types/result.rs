//! Task result definitions

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Result of a task execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskResult {
    /// Output data (format depends on task type)
    #[serde(default)]
    pub output: serde_json::Value,

    /// Files created or modified by this task
    #[serde(default)]
    pub artifacts: Vec<PathBuf>,

    /// Execution duration
    #[serde(default, with = "duration_serde")]
    pub duration: Duration,

    /// Optional summary message
    pub summary: Option<String>,
}

impl TaskResult {
    /// Create an empty result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a result with output
    pub fn with_output(output: serde_json::Value) -> Self {
        Self {
            output,
            ..Default::default()
        }
    }

    /// Create a result with string output
    pub fn with_string(output: impl Into<String>) -> Self {
        Self {
            output: serde_json::Value::String(output.into()),
            ..Default::default()
        }
    }

    /// Builder: add artifact
    pub fn add_artifact(mut self, path: PathBuf) -> Self {
        self.artifacts.push(path);
        self
    }

    /// Builder: set duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Builder: set summary
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// Custom serialization for Duration
mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

/// Summary of a completed task graph execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    /// ID of the executed graph
    pub graph_id: String,

    /// Total number of tasks
    pub total_tasks: usize,

    /// Number of completed tasks
    pub completed_tasks: usize,

    /// Number of failed tasks
    pub failed_tasks: usize,

    /// Number of cancelled tasks
    pub cancelled_tasks: usize,

    /// Total execution duration
    #[serde(with = "duration_serde")]
    pub total_duration: Duration,

    /// All artifacts produced
    pub artifacts: Vec<PathBuf>,

    /// Error messages from failed tasks
    pub errors: Vec<String>,
}

impl ExecutionSummary {
    /// Create a new summary
    pub fn new(graph_id: impl Into<String>) -> Self {
        Self {
            graph_id: graph_id.into(),
            total_tasks: 0,
            completed_tasks: 0,
            failed_tasks: 0,
            cancelled_tasks: 0,
            total_duration: Duration::ZERO,
            artifacts: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Check if execution was successful (no failures)
    pub fn is_success(&self) -> bool {
        self.failed_tasks == 0 && self.cancelled_tasks == 0
    }

    /// Get completion percentage
    pub fn completion_percentage(&self) -> f32 {
        if self.total_tasks == 0 {
            return 100.0;
        }
        (self.completed_tasks as f32 / self.total_tasks as f32) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_creation() {
        let result = TaskResult::with_string("Hello, world!")
            .with_duration(Duration::from_secs(5))
            .with_summary("Completed successfully");

        assert_eq!(
            result.output,
            serde_json::Value::String("Hello, world!".into())
        );
        assert_eq!(result.duration, Duration::from_secs(5));
        assert_eq!(result.summary, Some("Completed successfully".into()));
    }

    #[test]
    fn test_execution_summary() {
        let mut summary = ExecutionSummary::new("graph_1");
        summary.total_tasks = 5;
        summary.completed_tasks = 5;

        assert!(summary.is_success());
        assert_eq!(summary.completion_percentage(), 100.0);
    }

    #[test]
    fn test_execution_summary_with_failures() {
        let mut summary = ExecutionSummary::new("graph_2");
        summary.total_tasks = 10;
        summary.completed_tasks = 8;
        summary.failed_tasks = 2;
        summary.errors.push("Task failed: connection timeout".into());

        assert!(!summary.is_success());
        assert_eq!(summary.completion_percentage(), 80.0);
    }
}

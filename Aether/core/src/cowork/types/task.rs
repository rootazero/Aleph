//! Task type definitions

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use super::TaskResult;

/// A single task in the task graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier for this task
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Optional description
    pub description: Option<String>,

    /// Type of task (determines which executor handles it)
    pub task_type: TaskType,

    /// Task-specific parameters
    pub parameters: serde_json::Value,

    /// Preferred AI model for this task (if applicable)
    pub model_preference: Option<String>,

    /// Estimated duration (for progress calculation)
    pub estimated_duration: Option<Duration>,

    /// Current execution status
    #[serde(default)]
    pub status: TaskStatus,
}

impl Task {
    /// Create a new pending task
    pub fn new(id: impl Into<String>, name: impl Into<String>, task_type: TaskType) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            task_type,
            parameters: serde_json::Value::Null,
            model_preference: None,
            estimated_duration: None,
            status: TaskStatus::Pending,
        }
    }

    /// Builder: set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Builder: set parameters
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Builder: set model preference
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_preference = Some(model.into());
        self
    }

    /// Builder: set estimated duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.estimated_duration = Some(duration);
        self
    }

    /// Check if task is pending
    pub fn is_pending(&self) -> bool {
        matches!(self.status, TaskStatus::Pending)
    }

    /// Check if task is running
    pub fn is_running(&self) -> bool {
        matches!(self.status, TaskStatus::Running { .. })
    }

    /// Check if task is completed
    pub fn is_completed(&self) -> bool {
        matches!(self.status, TaskStatus::Completed { .. })
    }

    /// Check if task has failed
    pub fn is_failed(&self) -> bool {
        matches!(self.status, TaskStatus::Failed { .. })
    }

    /// Check if task is cancelled
    pub fn is_cancelled(&self) -> bool {
        matches!(self.status, TaskStatus::Cancelled)
    }

    /// Check if task is finished (completed, failed, or cancelled)
    pub fn is_finished(&self) -> bool {
        self.is_completed() || self.is_failed() || self.is_cancelled()
    }

    /// Get current progress (0.0 - 1.0)
    pub fn progress(&self) -> f32 {
        match &self.status {
            TaskStatus::Pending => 0.0,
            TaskStatus::Running { progress, .. } => *progress,
            TaskStatus::Completed { .. } => 1.0,
            TaskStatus::Failed { .. } => 0.0,
            TaskStatus::Cancelled => 0.0,
        }
    }
}

/// Type of task - determines which executor handles it
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskType {
    /// File system operations (read, write, move, search)
    FileOperation(FileOp),

    /// Code/script execution
    CodeExecution(CodeExec),

    /// Document generation (Excel, PowerPoint, PDF)
    DocumentGeneration(DocGen),

    /// macOS application automation
    AppAutomation(AppAuto),

    /// AI inference task
    AiInference(AiTask),
}

impl TaskType {
    /// Get the category name for this task type
    pub fn category(&self) -> &'static str {
        match self {
            TaskType::FileOperation(_) => "file_operation",
            TaskType::CodeExecution(_) => "code_execution",
            TaskType::DocumentGeneration(_) => "document_generation",
            TaskType::AppAutomation(_) => "app_automation",
            TaskType::AiInference(_) => "ai_inference",
        }
    }
}

/// File operation subtypes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    /// Read file contents
    Read { path: PathBuf },

    /// Write content to file
    Write { path: PathBuf },

    /// Move/rename file
    Move { from: PathBuf, to: PathBuf },

    /// Copy file
    Copy { from: PathBuf, to: PathBuf },

    /// Delete file
    Delete { path: PathBuf },

    /// Search for files
    Search { pattern: String, dir: PathBuf },

    /// List directory contents
    List { path: PathBuf },

    /// Batch move operations
    BatchMove { operations: Vec<(PathBuf, PathBuf)> },
}

/// Code execution subtypes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "exec", rename_all = "snake_case")]
pub enum CodeExec {
    /// Execute inline code
    Script { code: String, language: Language },

    /// Execute a script file
    File { path: PathBuf },

    /// Execute a shell command
    Command { cmd: String, args: Vec<String> },
}

/// Supported programming languages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    JavaScript,
    Shell,
    Ruby,
    Rust,
}

/// Document generation subtypes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum DocGen {
    /// Generate Excel spreadsheet
    Excel {
        template: Option<PathBuf>,
        output: PathBuf,
    },

    /// Generate PowerPoint presentation
    PowerPoint {
        template: Option<PathBuf>,
        output: PathBuf,
    },

    /// Generate PDF document
    Pdf {
        style: Option<String>,
        output: PathBuf,
    },

    /// Generate Markdown document
    Markdown { output: PathBuf },
}

/// Application automation subtypes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "auto_type", rename_all = "snake_case")]
pub enum AppAuto {
    /// Launch application
    Launch { bundle_id: String },

    /// Run AppleScript
    AppleScript { script: String },

    /// Perform UI action
    UiAction { ui_action: String, target: String },
}

/// AI inference task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiTask {
    /// The prompt to send to the AI
    pub prompt: String,

    /// Whether this task requires privacy (use local model)
    #[serde(default)]
    pub requires_privacy: bool,

    /// Whether the input contains images
    #[serde(default)]
    pub has_images: bool,

    /// Expected output format
    pub output_format: Option<String>,
}

/// Current execution status of a task
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is waiting to be executed
    #[default]
    Pending,

    /// Task is currently executing
    Running {
        /// Progress from 0.0 to 1.0
        progress: f32,
        /// Optional status message
        message: Option<String>,
    },

    /// Task completed successfully
    Completed {
        /// Execution result
        result: TaskResult,
    },

    /// Task failed
    Failed {
        /// Error message
        error: String,
        /// Whether the task can be retried
        recoverable: bool,
    },

    /// Task was cancelled
    Cancelled,
}

impl TaskStatus {
    /// Create a running status with progress
    pub fn running(progress: f32) -> Self {
        Self::Running {
            progress,
            message: None,
        }
    }

    /// Create a running status with progress and message
    pub fn running_with_message(progress: f32, message: impl Into<String>) -> Self {
        Self::Running {
            progress,
            message: Some(message.into()),
        }
    }

    /// Create a completed status
    pub fn completed(result: TaskResult) -> Self {
        Self::Completed { result }
    }

    /// Create a failed status
    pub fn failed(error: impl Into<String>) -> Self {
        Self::Failed {
            error: error.into(),
            recoverable: false,
        }
    }

    /// Create a recoverable failed status
    pub fn failed_recoverable(error: impl Into<String>) -> Self {
        Self::Failed {
            error: error.into(),
            recoverable: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = Task::new(
            "task_1",
            "Read config file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/etc/config"),
            }),
        );

        assert_eq!(task.id, "task_1");
        assert_eq!(task.name, "Read config file");
        assert!(task.is_pending());
        assert_eq!(task.progress(), 0.0);
    }

    #[test]
    fn test_task_builder() {
        let task = Task::new(
            "task_2",
            "AI Analysis",
            TaskType::AiInference(AiTask {
                prompt: "Analyze this text".into(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_description("Perform AI analysis")
        .with_model("claude-sonnet");

        assert_eq!(task.description, Some("Perform AI analysis".into()));
        assert_eq!(task.model_preference, Some("claude-sonnet".into()));
    }

    #[test]
    fn test_task_type_category() {
        let file_op = TaskType::FileOperation(FileOp::Read {
            path: PathBuf::from("/tmp"),
        });
        assert_eq!(file_op.category(), "file_operation");

        let ai_task = TaskType::AiInference(AiTask {
            prompt: "test".into(),
            requires_privacy: false,
            has_images: false,
            output_format: None,
        });
        assert_eq!(ai_task.category(), "ai_inference");
    }

    #[test]
    fn test_task_status_transitions() {
        let mut task = Task::new(
            "task_3",
            "Test task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        assert!(task.is_pending());

        task.status = TaskStatus::running(0.5);
        assert!(task.is_running());
        assert_eq!(task.progress(), 0.5);

        task.status = TaskStatus::completed(TaskResult::default());
        assert!(task.is_completed());
        assert!(task.is_finished());
        assert_eq!(task.progress(), 1.0);
    }
}

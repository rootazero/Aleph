//! Execution result types for the unified executor
//!
//! This module defines the core types for the unified executor architecture:
//! - `ExecutionResult`: The outcome of executing a plan
//! - `ToolCallRecord`: Record of a tool call execution
//! - `TaskExecutionResult`: Result of executing a single task within a TaskGraph
//! - `ExecutionContext`: Context for execution
//! - `ExecutorError`: Error types for executor operations

use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// Execution Result
// =============================================================================

/// Result of executing a plan
///
/// Represents the complete outcome of plan execution, including:
/// - Content: The final response or output
/// - Tool calls: All tool calls made during execution
/// - Task results: Results from individual tasks (for TaskGraph plans)
/// - Timing and success information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Final content/response from the execution
    pub content: String,

    /// All tool calls made during execution
    pub tool_calls: Vec<ToolCallRecord>,

    /// Results from individual tasks (for TaskGraph execution)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_results: Option<Vec<TaskExecutionResult>>,

    /// Total execution time in milliseconds
    pub execution_time_ms: u64,

    /// Whether the execution completed successfully
    pub success: bool,

    /// Error message if execution failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ExecutionResult {
    /// Create a successful execution result
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            tool_calls: Vec::new(),
            task_results: None,
            execution_time_ms: 0,
            success: true,
            error: None,
        }
    }

    /// Create a failed execution result
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            task_results: None,
            execution_time_ms: 0,
            success: false,
            error: Some(error.into()),
        }
    }

    /// Builder: add tool calls
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallRecord>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Builder: add task results
    pub fn with_task_results(mut self, results: Vec<TaskExecutionResult>) -> Self {
        self.task_results = Some(results);
        self
    }

    /// Builder: set execution time from Duration
    pub fn with_execution_time(mut self, time: Duration) -> Self {
        self.execution_time_ms = time.as_millis() as u64;
        self
    }

    /// Builder: set execution time in milliseconds
    pub fn with_execution_time_ms(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }

    /// Get the number of successful tool calls
    pub fn successful_tool_calls(&self) -> usize {
        self.tool_calls.iter().filter(|c| c.success).count()
    }

    /// Get the number of failed tool calls
    pub fn failed_tool_calls(&self) -> usize {
        self.tool_calls.iter().filter(|c| !c.success).count()
    }

    /// Check if any tool calls failed
    pub fn has_tool_failures(&self) -> bool {
        self.tool_calls.iter().any(|c| !c.success)
    }

    /// Get all tool names that were called
    pub fn tool_names(&self) -> Vec<&str> {
        self.tool_calls
            .iter()
            .map(|c| c.tool_name.as_str())
            .collect()
    }
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            task_results: None,
            execution_time_ms: 0,
            success: true,
            error: None,
        }
    }
}

// =============================================================================
// Tool Call Record
// =============================================================================

/// Record of a tool call during execution
///
/// Tracks all information about a single tool call:
/// - Tool name and parameters
/// - Result and success status
/// - Execution timing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Name of the tool that was called
    pub tool_name: String,

    /// Parameters passed to the tool
    pub parameters: serde_json::Value,

    /// Result content from the tool (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,

    /// Whether the tool call succeeded
    pub success: bool,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

impl ToolCallRecord {
    /// Create a new tool call record (pending execution)
    pub fn new(tool_name: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            parameters,
            result: None,
            success: false,
            execution_time_ms: 0,
        }
    }

    /// Builder: set the result and success status
    pub fn with_result(mut self, result: impl Into<String>, success: bool) -> Self {
        self.result = Some(result.into());
        self.success = success;
        self
    }

    /// Builder: set execution time from Duration
    pub fn with_execution_time(mut self, time: Duration) -> Self {
        self.execution_time_ms = time.as_millis() as u64;
        self
    }

    /// Builder: set execution time in milliseconds
    pub fn with_execution_time_ms(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }

    /// Create a successful tool call record
    pub fn success(
        tool_name: impl Into<String>,
        parameters: serde_json::Value,
        result: impl Into<String>,
        execution_time_ms: u64,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            parameters,
            result: Some(result.into()),
            success: true,
            execution_time_ms,
        }
    }

    /// Create a failed tool call record
    pub fn failure(
        tool_name: impl Into<String>,
        parameters: serde_json::Value,
        error: impl Into<String>,
        execution_time_ms: u64,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            parameters,
            result: Some(error.into()),
            success: false,
            execution_time_ms,
        }
    }
}

// =============================================================================
// Task Execution Result
// =============================================================================

/// Result of executing a single task within a TaskGraph
///
/// Used when executing multi-step plans to track the outcome
/// of each individual task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    /// Unique identifier for the task
    pub task_id: String,

    /// Human-readable description of what the task did
    pub description: String,

    /// Whether the task completed successfully
    pub success: bool,

    /// Output content from the task
    pub output: String,

    /// Error message if the task failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

impl TaskExecutionResult {
    /// Create a successful task result
    pub fn success(
        task_id: impl Into<String>,
        description: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            success: true,
            output: output.into(),
            error: None,
            execution_time_ms: 0,
        }
    }

    /// Create a failed task result
    pub fn failure(
        task_id: impl Into<String>,
        description: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            success: false,
            output: String::new(),
            error: Some(error.into()),
            execution_time_ms: 0,
        }
    }

    /// Builder: set execution time from Duration
    pub fn with_execution_time(mut self, time: Duration) -> Self {
        self.execution_time_ms = time.as_millis() as u64;
        self
    }

    /// Builder: set execution time in milliseconds
    pub fn with_execution_time_ms(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }
}

// =============================================================================
// Execution Context
// =============================================================================

/// Context for execution
///
/// Provides contextual information that may affect how execution
/// is performed, such as the current application or window.
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Current application context (e.g., "Safari", "VSCode")
    pub app_context: Option<String>,

    /// Current window title
    pub window_title: Option<String>,

    /// Topic/conversation ID for memory association
    pub topic_id: Option<String>,

    /// Whether to stream the response (defaults to true)
    pub stream: bool,
}

impl ExecutionContext {
    /// Create a new execution context with default values
    ///
    /// Note: stream defaults to true for optimal user experience
    pub fn new() -> Self {
        Self {
            app_context: None,
            window_title: None,
            topic_id: None,
            stream: true, // Default to streaming
        }
    }

    /// Builder: set application context
    pub fn with_app_context(mut self, ctx: impl Into<String>) -> Self {
        self.app_context = Some(ctx.into());
        self
    }

    /// Builder: set window title
    pub fn with_window_title(mut self, title: impl Into<String>) -> Self {
        self.window_title = Some(title.into());
        self
    }

    /// Builder: set topic ID
    pub fn with_topic_id(mut self, id: impl Into<String>) -> Self {
        self.topic_id = Some(id.into());
        self
    }

    /// Builder: set streaming mode
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Check if this context has any application information
    pub fn has_app_info(&self) -> bool {
        self.app_context.is_some() || self.window_title.is_some()
    }

    /// Get a combined app description
    pub fn app_description(&self) -> Option<String> {
        match (&self.app_context, &self.window_title) {
            (Some(app), Some(title)) => Some(format!("{} - {}", app, title)),
            (Some(app), None) => Some(app.clone()),
            (None, Some(title)) => Some(title.clone()),
            (None, None) => None,
        }
    }
}

// =============================================================================
// Executor Error
// =============================================================================

/// Executor error types
///
/// Represents the various ways execution can fail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutorError {
    /// General execution failure
    ExecutionFailed(String),

    /// Tool execution error
    ToolError(String),

    /// Task execution failed
    TaskFailed {
        /// ID of the task that failed
        task_id: String,
        /// Error message
        error: String,
    },

    /// Execution timed out
    Timeout,

    /// Execution was cancelled
    Cancelled,
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            ExecutorError::ToolError(msg) => write!(f, "Tool error: {}", msg),
            ExecutorError::TaskFailed { task_id, error } => {
                write!(f, "Task {} failed: {}", task_id, error)
            }
            ExecutorError::Timeout => write!(f, "Execution timed out"),
            ExecutorError::Cancelled => write!(f, "Execution was cancelled"),
        }
    }
}

impl std::error::Error for ExecutorError {}

impl ExecutorError {
    /// Create an execution failed error
    pub fn execution_failed(msg: impl Into<String>) -> Self {
        ExecutorError::ExecutionFailed(msg.into())
    }

    /// Create a tool error
    pub fn tool_error(msg: impl Into<String>) -> Self {
        ExecutorError::ToolError(msg.into())
    }

    /// Create a task failed error
    pub fn task_failed(task_id: impl Into<String>, error: impl Into<String>) -> Self {
        ExecutorError::TaskFailed {
            task_id: task_id.into(),
            error: error.into(),
        }
    }

    /// Check if this error is recoverable
    ///
    /// Recoverable errors may be worth retrying.
    pub fn is_recoverable(&self) -> bool {
        match self {
            ExecutorError::ExecutionFailed(_) => true, // May be transient
            ExecutorError::ToolError(_) => true,       // Tool may work on retry
            ExecutorError::TaskFailed { .. } => true,  // Task may succeed on retry
            ExecutorError::Timeout => true,            // Can retry with longer timeout
            ExecutorError::Cancelled => false,         // User cancelled, don't retry
        }
    }

    /// Check if this error is due to timeout
    pub fn is_timeout(&self) -> bool {
        matches!(self, ExecutorError::Timeout)
    }

    /// Check if this error is due to cancellation
    pub fn is_cancelled(&self) -> bool {
        matches!(self, ExecutorError::Cancelled)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // ExecutionResult Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult::success("Hello, world!");

        assert!(result.success);
        assert_eq!(result.content, "Hello, world!");
        assert!(result.error.is_none());
        assert!(result.tool_calls.is_empty());
        assert!(result.task_results.is_none());
    }

    #[test]
    fn test_execution_result_failure() {
        let result = ExecutionResult::failure("Something went wrong");

        assert!(!result.success);
        assert!(result.content.is_empty());
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_execution_result_builders() {
        let tool_calls = vec![ToolCallRecord::new(
            "search",
            serde_json::json!({"query": "test"}),
        )];

        let task_results = vec![TaskExecutionResult::success(
            "task_1",
            "Test task",
            "Output",
        )];

        let result = ExecutionResult::success("Done")
            .with_tool_calls(tool_calls.clone())
            .with_task_results(task_results.clone())
            .with_execution_time(Duration::from_millis(150));

        assert_eq!(result.tool_calls.len(), 1);
        assert!(result.task_results.is_some());
        assert_eq!(result.task_results.unwrap().len(), 1);
        assert_eq!(result.execution_time_ms, 150);
    }

    #[test]
    fn test_execution_result_with_execution_time_ms() {
        let result = ExecutionResult::success("Done").with_execution_time_ms(500);
        assert_eq!(result.execution_time_ms, 500);
    }

    #[test]
    fn test_execution_result_default() {
        let result = ExecutionResult::default();

        assert!(result.success);
        assert!(result.content.is_empty());
        assert!(result.tool_calls.is_empty());
        assert!(result.task_results.is_none());
        assert_eq!(result.execution_time_ms, 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_execution_result_tool_call_counts() {
        let tool_calls = vec![
            ToolCallRecord::success("tool_1", serde_json::json!({}), "success", 100),
            ToolCallRecord::failure("tool_2", serde_json::json!({}), "error", 50),
            ToolCallRecord::success("tool_3", serde_json::json!({}), "success", 75),
        ];

        let result = ExecutionResult::success("Done").with_tool_calls(tool_calls);

        assert_eq!(result.successful_tool_calls(), 2);
        assert_eq!(result.failed_tool_calls(), 1);
        assert!(result.has_tool_failures());
    }

    #[test]
    fn test_execution_result_tool_names() {
        let tool_calls = vec![
            ToolCallRecord::new("search", serde_json::json!({})),
            ToolCallRecord::new("read_file", serde_json::json!({})),
            ToolCallRecord::new("write_file", serde_json::json!({})),
        ];

        let result = ExecutionResult::success("Done").with_tool_calls(tool_calls);
        let names = result.tool_names();

        assert_eq!(names, vec!["search", "read_file", "write_file"]);
    }

    #[test]
    fn test_execution_result_no_tool_failures() {
        let tool_calls = vec![
            ToolCallRecord::success("tool_1", serde_json::json!({}), "ok", 100),
            ToolCallRecord::success("tool_2", serde_json::json!({}), "ok", 100),
        ];

        let result = ExecutionResult::success("Done").with_tool_calls(tool_calls);
        assert!(!result.has_tool_failures());
    }

    #[test]
    fn test_execution_result_serialization() {
        let result = ExecutionResult::success("Test content")
            .with_execution_time_ms(100)
            .with_tool_calls(vec![ToolCallRecord::success(
                "test_tool",
                serde_json::json!({"arg": "value"}),
                "result",
                50,
            )]);

        let json = serde_json::to_string(&result).expect("Serialization should succeed");
        let deserialized: ExecutionResult =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized.content, "Test content");
        assert_eq!(deserialized.execution_time_ms, 100);
        assert_eq!(deserialized.tool_calls.len(), 1);
        assert!(deserialized.success);
    }

    // -------------------------------------------------------------------------
    // ToolCallRecord Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tool_call_record_new() {
        let params = serde_json::json!({"path": "/tmp/test.txt"});
        let record = ToolCallRecord::new("read_file", params.clone());

        assert_eq!(record.tool_name, "read_file");
        assert_eq!(record.parameters, params);
        assert!(record.result.is_none());
        assert!(!record.success);
        assert_eq!(record.execution_time_ms, 0);
    }

    #[test]
    fn test_tool_call_record_with_result() {
        let record = ToolCallRecord::new("search", serde_json::json!({"query": "test"}))
            .with_result("Found 5 results", true);

        assert!(record.success);
        assert_eq!(record.result, Some("Found 5 results".to_string()));
    }

    #[test]
    fn test_tool_call_record_with_execution_time() {
        let record = ToolCallRecord::new("tool", serde_json::json!({}))
            .with_execution_time(Duration::from_secs(2));

        assert_eq!(record.execution_time_ms, 2000);
    }

    #[test]
    fn test_tool_call_record_with_execution_time_ms() {
        let record =
            ToolCallRecord::new("tool", serde_json::json!({})).with_execution_time_ms(1500);

        assert_eq!(record.execution_time_ms, 1500);
    }

    #[test]
    fn test_tool_call_record_success() {
        let params = serde_json::json!({"query": "rust"});
        let record = ToolCallRecord::success("search", params.clone(), "Found results", 250);

        assert_eq!(record.tool_name, "search");
        assert_eq!(record.parameters, params);
        assert!(record.success);
        assert_eq!(record.result, Some("Found results".to_string()));
        assert_eq!(record.execution_time_ms, 250);
    }

    #[test]
    fn test_tool_call_record_failure() {
        let params = serde_json::json!({"path": "/nonexistent"});
        let record = ToolCallRecord::failure("read_file", params.clone(), "File not found", 50);

        assert_eq!(record.tool_name, "read_file");
        assert!(!record.success);
        assert_eq!(record.result, Some("File not found".to_string()));
    }

    #[test]
    fn test_tool_call_record_serialization() {
        let record = ToolCallRecord::success(
            "test_tool",
            serde_json::json!({"key": "value"}),
            "result data",
            100,
        );

        let json = serde_json::to_string(&record).expect("Serialization should succeed");
        let deserialized: ToolCallRecord =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized.tool_name, "test_tool");
        assert!(deserialized.success);
        assert_eq!(deserialized.result, Some("result data".to_string()));
    }

    // -------------------------------------------------------------------------
    // TaskExecutionResult Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_task_execution_result_success() {
        let result = TaskExecutionResult::success("task_1", "Process data", "Data processed");

        assert_eq!(result.task_id, "task_1");
        assert_eq!(result.description, "Process data");
        assert!(result.success);
        assert_eq!(result.output, "Data processed");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_task_execution_result_failure() {
        let result = TaskExecutionResult::failure("task_2", "Read config", "File not found");

        assert_eq!(result.task_id, "task_2");
        assert_eq!(result.description, "Read config");
        assert!(!result.success);
        assert!(result.output.is_empty());
        assert_eq!(result.error, Some("File not found".to_string()));
    }

    #[test]
    fn test_task_execution_result_with_execution_time() {
        let result = TaskExecutionResult::success("task_1", "Test", "Output")
            .with_execution_time(Duration::from_millis(500));

        assert_eq!(result.execution_time_ms, 500);
    }

    #[test]
    fn test_task_execution_result_with_execution_time_ms() {
        let result =
            TaskExecutionResult::success("task_1", "Test", "Output").with_execution_time_ms(750);

        assert_eq!(result.execution_time_ms, 750);
    }

    #[test]
    fn test_task_execution_result_serialization() {
        let result = TaskExecutionResult::success("task_1", "Test task", "Success output")
            .with_execution_time_ms(200);

        let json = serde_json::to_string(&result).expect("Serialization should succeed");
        let deserialized: TaskExecutionResult =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized.task_id, "task_1");
        assert!(deserialized.success);
        assert_eq!(deserialized.output, "Success output");
    }

    // -------------------------------------------------------------------------
    // ExecutionContext Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_execution_context_new() {
        let ctx = ExecutionContext::new();

        assert!(ctx.app_context.is_none());
        assert!(ctx.window_title.is_none());
        assert!(ctx.topic_id.is_none());
        assert!(ctx.stream); // Default is true
    }

    #[test]
    fn test_execution_context_default() {
        let ctx = ExecutionContext::default();

        // Default should NOT stream (different from new())
        assert!(!ctx.stream);
    }

    #[test]
    fn test_execution_context_builders() {
        let ctx = ExecutionContext::new()
            .with_app_context("VSCode")
            .with_window_title("main.rs - Aether")
            .with_topic_id("topic_123")
            .with_stream(false);

        assert_eq!(ctx.app_context, Some("VSCode".to_string()));
        assert_eq!(ctx.window_title, Some("main.rs - Aether".to_string()));
        assert_eq!(ctx.topic_id, Some("topic_123".to_string()));
        assert!(!ctx.stream);
    }

    #[test]
    fn test_execution_context_has_app_info() {
        let ctx1 = ExecutionContext::new();
        assert!(!ctx1.has_app_info());

        let ctx2 = ExecutionContext::new().with_app_context("Safari");
        assert!(ctx2.has_app_info());

        let ctx3 = ExecutionContext::new().with_window_title("Google");
        assert!(ctx3.has_app_info());
    }

    #[test]
    fn test_execution_context_app_description() {
        let ctx1 = ExecutionContext::new();
        assert!(ctx1.app_description().is_none());

        let ctx2 = ExecutionContext::new().with_app_context("Safari");
        assert_eq!(ctx2.app_description(), Some("Safari".to_string()));

        let ctx3 = ExecutionContext::new().with_window_title("Google Search");
        assert_eq!(ctx3.app_description(), Some("Google Search".to_string()));

        let ctx4 = ExecutionContext::new()
            .with_app_context("Safari")
            .with_window_title("Google Search");
        assert_eq!(
            ctx4.app_description(),
            Some("Safari - Google Search".to_string())
        );
    }

    // -------------------------------------------------------------------------
    // ExecutorError Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_executor_error_display() {
        assert_eq!(
            ExecutorError::ExecutionFailed("Network error".to_string()).to_string(),
            "Execution failed: Network error"
        );

        assert_eq!(
            ExecutorError::ToolError("Invalid params".to_string()).to_string(),
            "Tool error: Invalid params"
        );

        assert_eq!(
            ExecutorError::TaskFailed {
                task_id: "task_1".to_string(),
                error: "File missing".to_string()
            }
            .to_string(),
            "Task task_1 failed: File missing"
        );

        assert_eq!(ExecutorError::Timeout.to_string(), "Execution timed out");

        assert_eq!(
            ExecutorError::Cancelled.to_string(),
            "Execution was cancelled"
        );
    }

    #[test]
    fn test_executor_error_constructors() {
        let err1 = ExecutorError::execution_failed("Connection lost");
        assert_eq!(
            err1,
            ExecutorError::ExecutionFailed("Connection lost".to_string())
        );

        let err2 = ExecutorError::tool_error("Invalid argument");
        assert_eq!(
            err2,
            ExecutorError::ToolError("Invalid argument".to_string())
        );

        let err3 = ExecutorError::task_failed("task_42", "Dependency failed");
        assert_eq!(
            err3,
            ExecutorError::TaskFailed {
                task_id: "task_42".to_string(),
                error: "Dependency failed".to_string()
            }
        );
    }

    #[test]
    fn test_executor_error_is_recoverable() {
        assert!(ExecutorError::ExecutionFailed("Error".to_string()).is_recoverable());
        assert!(ExecutorError::ToolError("Error".to_string()).is_recoverable());
        assert!(ExecutorError::TaskFailed {
            task_id: "t".to_string(),
            error: "e".to_string()
        }
        .is_recoverable());
        assert!(ExecutorError::Timeout.is_recoverable());
        assert!(!ExecutorError::Cancelled.is_recoverable()); // Cancelled is NOT recoverable
    }

    #[test]
    fn test_executor_error_is_timeout() {
        assert!(ExecutorError::Timeout.is_timeout());
        assert!(!ExecutorError::Cancelled.is_timeout());
        assert!(!ExecutorError::ExecutionFailed("e".to_string()).is_timeout());
    }

    #[test]
    fn test_executor_error_is_cancelled() {
        assert!(ExecutorError::Cancelled.is_cancelled());
        assert!(!ExecutorError::Timeout.is_cancelled());
        assert!(!ExecutorError::ExecutionFailed("e".to_string()).is_cancelled());
    }

    #[test]
    fn test_executor_error_serialization() {
        let errors = vec![
            ExecutorError::ExecutionFailed("Test error".to_string()),
            ExecutorError::ToolError("Tool failed".to_string()),
            ExecutorError::TaskFailed {
                task_id: "t1".to_string(),
                error: "Failed".to_string(),
            },
            ExecutorError::Timeout,
            ExecutorError::Cancelled,
        ];

        for error in errors {
            let json = serde_json::to_string(&error).expect("Serialization should succeed");
            let deserialized: ExecutorError =
                serde_json::from_str(&json).expect("Deserialization should succeed");
            assert_eq!(deserialized, error);
        }
    }

    #[test]
    fn test_executor_error_std_error_trait() {
        let error: Box<dyn std::error::Error> =
            Box::new(ExecutorError::ExecutionFailed("Test".to_string()));
        assert!(error.to_string().contains("Execution failed"));
    }

    // -------------------------------------------------------------------------
    // Integration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_complex_execution_result() {
        // Simulate a real execution scenario
        let tool_calls = vec![
            ToolCallRecord::success(
                "list_files",
                serde_json::json!({"path": "/tmp"}),
                "file1.txt, file2.txt",
                50,
            ),
            ToolCallRecord::success(
                "read_file",
                serde_json::json!({"path": "/tmp/file1.txt"}),
                "Hello, world!",
                25,
            ),
            ToolCallRecord::failure(
                "write_file",
                serde_json::json!({"path": "/protected/file.txt", "content": "test"}),
                "Permission denied",
                10,
            ),
        ];

        let task_results = vec![
            TaskExecutionResult::success("task_0", "List files", "Found 2 files")
                .with_execution_time_ms(50),
            TaskExecutionResult::success("task_1", "Read file", "Hello, world!")
                .with_execution_time_ms(25),
            TaskExecutionResult::failure("task_2", "Write file", "Permission denied")
                .with_execution_time_ms(10),
        ];

        let result = ExecutionResult::success("Completed with errors")
            .with_tool_calls(tool_calls)
            .with_task_results(task_results)
            .with_execution_time_ms(85);

        // Verify counts
        assert_eq!(result.successful_tool_calls(), 2);
        assert_eq!(result.failed_tool_calls(), 1);
        assert!(result.has_tool_failures());

        // Verify tool names
        let names = result.tool_names();
        assert_eq!(names, vec!["list_files", "read_file", "write_file"]);

        // Verify task results
        let tasks = result.task_results.as_ref().unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].success);
        assert!(tasks[1].success);
        assert!(!tasks[2].success);

        // Verify serialization round-trip
        let json = serde_json::to_string(&result).unwrap();
        let _: ExecutionResult = serde_json::from_str(&json).unwrap();
    }
}

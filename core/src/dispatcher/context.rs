//! Task Context Management for DAG Scheduler
//!
//! This module provides context passing between tasks in the DAG scheduler:
//! - **Implicit History**: Accumulated history of task executions
//! - **Explicit Variables**: Named outputs that can be referenced by task ID
//!
//! # Example
//!
//! ```rust
//! use aethecore::dispatcher::{TaskContext, TaskOutput};
//!
//! let mut ctx = TaskContext::new("Analyze and summarize the document");
//!
//! // Record task output
//! ctx.record_output("task_1", TaskOutput::text("Document contains 5 sections"));
//!
//! // Build prompt context for downstream task
//! let prompt = ctx.build_prompt_context("task_2", &["task_1"]);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Output type classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputType {
    /// Plain text output
    Text,
    /// Structured JSON output
    Json,
    /// Binary data (stored as base64)
    Binary,
    /// Error output
    Error,
}

impl Default for OutputType {
    fn default() -> Self {
        Self::Text
    }
}

/// Task output with value, summary, and type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutput {
    /// The actual output value
    pub value: serde_json::Value,
    /// Human-readable summary (used for display and prompt context)
    pub summary: Option<String>,
    /// Output type classification
    pub output_type: OutputType,
}

impl TaskOutput {
    /// Create a text output
    pub fn text(content: impl Into<String>) -> Self {
        let content = content.into();
        let summary = Self::create_summary(&content);
        Self {
            value: serde_json::Value::String(content),
            summary: Some(summary),
            output_type: OutputType::Text,
        }
    }

    /// Create a JSON output
    pub fn json(value: serde_json::Value) -> Self {
        let summary = Self::summarize_json(&value);
        Self {
            value,
            summary: Some(summary),
            output_type: OutputType::Json,
        }
    }

    /// Create an error output
    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            value: serde_json::Value::String(message.clone()),
            summary: Some(format!("Error: {}", Self::truncate(&message, 100))),
            output_type: OutputType::Error,
        }
    }

    /// Create a binary output (stored as base64)
    pub fn binary(data: &[u8]) -> Self {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        Self {
            value: serde_json::Value::String(encoded),
            summary: Some(format!("[Binary data: {} bytes]", data.len())),
            output_type: OutputType::Binary,
        }
    }

    /// Get the summary, or generate one from the value
    pub fn get_summary(&self) -> String {
        self.summary.clone().unwrap_or_else(|| match &self.value {
            serde_json::Value::String(s) => Self::create_summary(s),
            v => Self::summarize_json(v),
        })
    }

    /// Create a summary from text content (truncate if too long)
    fn create_summary(content: &str) -> String {
        Self::truncate(content, 200)
    }

    /// Summarize JSON value
    fn summarize_json(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => Self::truncate(s, 200),
            serde_json::Value::Array(arr) => format!("[Array with {} items]", arr.len()),
            serde_json::Value::Object(obj) => {
                let keys: Vec<_> = obj.keys().take(5).cloned().collect();
                if obj.len() > 5 {
                    format!("{{Object with keys: {}, ... ({} more)}}", keys.join(", "), obj.len() - 5)
                } else {
                    format!("{{Object with keys: {}}}", keys.join(", "))
                }
            }
        }
    }

    /// Truncate string to max length with ellipsis
    fn truncate(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            s.to_string()
        } else {
            format!("{}...", &s[..max_len.saturating_sub(3)])
        }
    }
}

/// History entry for implicit accumulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Task ID
    pub task_id: String,
    /// Task name (human-readable)
    pub task_name: String,
    /// Task output
    pub output: TaskOutput,
}

/// Task context manager for inter-task context passing
///
/// Supports two modes of context passing:
/// 1. **Implicit History**: All task outputs are accumulated in history
/// 2. **Explicit Reference**: Tasks can reference specific outputs by task ID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// Implicit history accumulation (most recent first)
    history: Vec<HistoryEntry>,
    /// Explicit variables (task_id -> output)
    variables: HashMap<String, TaskOutput>,
    /// Original user input
    user_input: String,
    /// Maximum history entries to keep
    max_history: usize,
}

impl TaskContext {
    /// Default maximum history entries
    pub const DEFAULT_MAX_HISTORY: usize = 5;

    /// Create a new task context with user input
    pub fn new(user_input: impl Into<String>) -> Self {
        Self {
            history: Vec::new(),
            variables: HashMap::new(),
            user_input: user_input.into(),
            max_history: Self::DEFAULT_MAX_HISTORY,
        }
    }

    /// Set maximum history entries (builder pattern)
    pub fn with_max_history(mut self, max: usize) -> Self {
        self.max_history = max;
        self
    }

    /// Get the original user input
    pub fn user_input(&self) -> &str {
        &self.user_input
    }

    /// Record task output (both in history and variables)
    pub fn record_output(&mut self, task_id: &str, output: TaskOutput) {
        self.record_output_with_name(task_id, task_id, output);
    }

    /// Record task output with a human-readable name
    pub fn record_output_with_name(&mut self, task_id: &str, task_name: &str, output: TaskOutput) {
        // Add to variables for explicit reference
        self.variables.insert(task_id.to_string(), output.clone());

        // Add to history for implicit accumulation
        self.history.push(HistoryEntry {
            task_id: task_id.to_string(),
            task_name: task_name.to_string(),
            output,
        });

        // Trim history if exceeds max
        if self.history.len() > self.max_history {
            let excess = self.history.len() - self.max_history;
            self.history.drain(0..excess);
        }
    }

    /// Get output by task ID
    pub fn get_output(&self, task_id: &str) -> Option<&TaskOutput> {
        self.variables.get(task_id)
    }

    /// Build prompt context for a task
    ///
    /// # Arguments
    /// * `task_id` - Current task ID
    /// * `dependencies` - List of dependency task IDs
    ///
    /// # Returns
    /// Formatted prompt context string
    pub fn build_prompt_context(&self, task_id: &str, dependencies: &[&str]) -> String {
        let mut parts = Vec::new();

        // 1. User original request
        parts.push(format!("用户原始请求: {}", self.user_input));

        // 2. Dependency results (explicit reference)
        if !dependencies.is_empty() {
            parts.push(String::new()); // Empty line
            parts.push("=== 前置任务结果 ===".to_string());
            for dep_id in dependencies {
                if let Some(output) = self.variables.get(*dep_id) {
                    parts.push(format!("[{}]: {}", dep_id, output.get_summary()));
                } else {
                    parts.push(format!("[{}]: (未找到输出)", dep_id));
                }
            }
        }

        // 3. Execution history (implicit accumulation)
        if !self.history.is_empty() {
            parts.push(String::new()); // Empty line
            parts.push("=== 执行历史 ===".to_string());
            for entry in &self.history {
                parts.push(format!("- {}: {}", entry.task_name, entry.output.get_summary()));
            }
        }

        // 4. Current task
        parts.push(String::new()); // Empty line
        parts.push("=== 当前任务 ===".to_string());
        parts.push(format!("任务ID: {}", task_id));

        parts.join("\n")
    }

    /// Clear all context
    pub fn clear(&mut self) {
        self.history.clear();
        self.variables.clear();
    }

    /// Get history entries
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// Get all variables
    pub fn variables(&self) -> &HashMap<String, TaskOutput> {
        &self.variables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_output_text() {
        let output = TaskOutput::text("Hello, world!");
        assert_eq!(output.output_type, OutputType::Text);
        assert_eq!(output.value, serde_json::Value::String("Hello, world!".to_string()));
        assert_eq!(output.summary, Some("Hello, world!".to_string()));
    }

    #[test]
    fn test_task_output_long_text() {
        // Create a string longer than 200 characters
        let long_text = "a".repeat(300);
        let output = TaskOutput::text(&long_text);

        // Summary should be truncated to 200 chars (197 + "...")
        let summary = output.summary.unwrap();
        assert!(summary.len() <= 200);
        assert!(summary.ends_with("..."));

        // Value should preserve full content
        assert_eq!(output.value, serde_json::Value::String(long_text));
    }

    #[test]
    fn test_task_output_json() {
        let json = serde_json::json!({
            "name": "test",
            "count": 42
        });
        let output = TaskOutput::json(json.clone());
        assert_eq!(output.output_type, OutputType::Json);
        assert_eq!(output.value, json);
        assert!(output.summary.unwrap().contains("Object"));
    }

    #[test]
    fn test_task_output_error() {
        let output = TaskOutput::error("Something went wrong");
        assert_eq!(output.output_type, OutputType::Error);
        assert!(output.summary.unwrap().starts_with("Error:"));
    }

    #[test]
    fn test_context_record_and_build() {
        let mut ctx = TaskContext::new("Analyze the document and generate summary");

        // Record first task output
        ctx.record_output_with_name("task_1", "Document Analysis", TaskOutput::text("Found 5 sections"));

        // Record second task output
        ctx.record_output_with_name("task_2", "Section Extraction", TaskOutput::text("Extracted key points"));

        // Build prompt for task_3 with dependencies on task_1 and task_2
        let prompt = ctx.build_prompt_context("task_3", &["task_1", "task_2"]);

        // Verify prompt contains expected sections
        assert!(prompt.contains("用户原始请求: Analyze the document"));
        assert!(prompt.contains("=== 前置任务结果 ==="));
        assert!(prompt.contains("[task_1]: Found 5 sections"));
        assert!(prompt.contains("[task_2]: Extracted key points"));
        assert!(prompt.contains("=== 执行历史 ==="));
        assert!(prompt.contains("- Document Analysis:"));
        assert!(prompt.contains("=== 当前任务 ==="));
        assert!(prompt.contains("任务ID: task_3"));
    }

    #[test]
    fn test_context_explicit_reference() {
        let mut ctx = TaskContext::new("Test input");

        ctx.record_output("analysis", TaskOutput::text("Analysis result"));
        ctx.record_output("summary", TaskOutput::text("Summary result"));

        // Explicit reference should work
        let analysis = ctx.get_output("analysis").unwrap();
        assert_eq!(analysis.value, serde_json::Value::String("Analysis result".to_string()));

        let summary = ctx.get_output("summary").unwrap();
        assert_eq!(summary.value, serde_json::Value::String("Summary result".to_string()));
    }

    #[test]
    fn test_context_get_output() {
        let mut ctx = TaskContext::new("Test");

        // Non-existent task should return None
        assert!(ctx.get_output("nonexistent").is_none());

        // After recording, should return Some
        ctx.record_output("task_1", TaskOutput::text("Result"));
        assert!(ctx.get_output("task_1").is_some());

        // Clear should remove all
        ctx.clear();
        assert!(ctx.get_output("task_1").is_none());
    }

    #[test]
    fn test_context_max_history() {
        let mut ctx = TaskContext::new("Test").with_max_history(3);

        // Record 5 tasks
        for i in 1..=5 {
            ctx.record_output(&format!("task_{}", i), TaskOutput::text(format!("Result {}", i)));
        }

        // History should only keep last 3
        assert_eq!(ctx.history().len(), 3);

        // Should have task_3, task_4, task_5
        assert_eq!(ctx.history()[0].task_id, "task_3");
        assert_eq!(ctx.history()[1].task_id, "task_4");
        assert_eq!(ctx.history()[2].task_id, "task_5");

        // But variables should keep all
        assert_eq!(ctx.variables().len(), 5);
    }

    #[test]
    fn test_context_user_input() {
        let ctx = TaskContext::new("Original user request");
        assert_eq!(ctx.user_input(), "Original user request");
    }

    #[test]
    fn test_output_type_default() {
        assert_eq!(OutputType::default(), OutputType::Text);
    }

    #[test]
    fn test_task_output_binary() {
        let data = b"Hello binary";
        let output = TaskOutput::binary(data);
        assert_eq!(output.output_type, OutputType::Binary);
        assert!(output.summary.unwrap().contains("12 bytes"));
    }

    #[test]
    fn test_build_prompt_no_dependencies() {
        let mut ctx = TaskContext::new("Simple task");
        ctx.record_output("prev_task", TaskOutput::text("Previous result"));

        // Build prompt with no dependencies
        let prompt = ctx.build_prompt_context("current_task", &[]);

        // Should not contain dependency section header if no deps
        assert!(prompt.contains("用户原始请求: Simple task"));
        assert!(prompt.contains("=== 执行历史 ==="));
        assert!(prompt.contains("=== 当前任务 ==="));
        assert!(prompt.contains("任务ID: current_task"));
    }

    #[test]
    fn test_build_prompt_missing_dependency() {
        let ctx = TaskContext::new("Test");

        // Build prompt with non-existent dependency
        let prompt = ctx.build_prompt_context("task_1", &["nonexistent"]);

        // Should show "未找到输出" for missing dependency
        assert!(prompt.contains("[nonexistent]: (未找到输出)"));
    }
}

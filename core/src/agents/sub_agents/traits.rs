//! Sub-Agent Traits and Core Types
//!
//! Defines the interface for all sub-agents in the delegation system.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Capability that a sub-agent can provide
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubAgentCapability {
    /// Can execute MCP tools from external servers
    McpToolExecution,
    /// Can execute skills (DAG workflows)
    SkillExecution,
    /// Can perform web searches
    WebSearch,
    /// Can perform file operations
    FileOperations,
    /// Can execute code
    CodeExecution,
    /// Can generate media (images, video, audio)
    MediaGeneration,
}

impl fmt::Display for SubAgentCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::McpToolExecution => write!(f, "mcp_tool_execution"),
            Self::SkillExecution => write!(f, "skill_execution"),
            Self::WebSearch => write!(f, "web_search"),
            Self::FileOperations => write!(f, "file_operations"),
            Self::CodeExecution => write!(f, "code_execution"),
            Self::MediaGeneration => write!(f, "media_generation"),
        }
    }
}

/// Request to a sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRequest {
    /// Unique request ID
    pub id: String,
    /// The prompt/task for the sub-agent
    pub prompt: String,
    /// Optional target (e.g., MCP server name, skill ID)
    pub target: Option<String>,
    /// Additional context from parent agent
    pub context: HashMap<String, Value>,
    /// Maximum iterations for the sub-agent
    pub max_iterations: Option<u32>,
    /// Parent session ID for tracking
    pub parent_session_id: Option<String>,
    /// Execution context from main agent
    pub execution_context: Option<ExecutionContextInfo>,
}

/// Execution context passed from main agent to sub-agents
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionContextInfo {
    /// Working directory for file operations
    pub working_directory: Option<String>,
    /// Current application context
    pub current_app: Option<String>,
    /// Window title context
    pub window_title: Option<String>,
    /// Original user request (for understanding intent)
    pub original_request: Option<String>,
    /// Summary of what has been done so far
    pub history_summary: Option<String>,
    /// Recent step summaries for context
    pub recent_steps: Vec<StepContextInfo>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Summary of a recent step for context passing
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StepContextInfo {
    /// What action was taken
    pub action_type: String,
    /// Brief description
    pub description: String,
    /// Whether it succeeded
    pub success: bool,
}

impl StepContextInfo {
    /// Create a new step context info
    pub fn new(
        action_type: impl Into<String>,
        description: impl Into<String>,
        success: bool,
    ) -> Self {
        Self {
            action_type: action_type.into(),
            description: description.into(),
            success,
        }
    }

    /// Create a successful step
    pub fn success(action_type: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(action_type, description, true)
    }

    /// Create a failed step
    pub fn failure(action_type: impl Into<String>, description: impl Into<String>) -> Self {
        Self::new(action_type, description, false)
    }
}

impl ExecutionContextInfo {
    /// Create new execution context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set working directory
    pub fn with_working_directory(mut self, dir: impl Into<String>) -> Self {
        self.working_directory = Some(dir.into());
        self
    }

    /// Set current app
    pub fn with_current_app(mut self, app: impl Into<String>) -> Self {
        self.current_app = Some(app.into());
        self
    }

    /// Set window title
    pub fn with_window_title(mut self, title: impl Into<String>) -> Self {
        self.window_title = Some(title.into());
        self
    }

    /// Set original request
    pub fn with_original_request(mut self, request: impl Into<String>) -> Self {
        self.original_request = Some(request.into());
        self
    }

    /// Set history summary
    pub fn with_history_summary(mut self, summary: impl Into<String>) -> Self {
        self.history_summary = Some(summary.into());
        self
    }

    /// Add a recent step
    pub fn with_step(mut self, step: StepContextInfo) -> Self {
        self.recent_steps.push(step);
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Check if context is empty
    pub fn is_empty(&self) -> bool {
        self.working_directory.is_none()
            && self.current_app.is_none()
            && self.window_title.is_none()
            && self.original_request.is_none()
            && self.history_summary.is_none()
            && self.recent_steps.is_empty()
            && self.metadata.is_empty()
    }

    /// Build a summary string from the context
    ///
    /// Creates a human-readable summary suitable for passing to sub-agents.
    /// The summary includes the history and recent steps in a concise format.
    ///
    /// # Arguments
    ///
    /// * `max_length` - Maximum length of the summary (default: 500 chars)
    pub fn build_summary(&self, max_length: Option<usize>) -> String {
        let max_len = max_length.unwrap_or(500);
        let mut parts = Vec::new();

        // Include existing history summary if present
        if let Some(ref history) = self.history_summary {
            parts.push(history.clone());
        }

        // Add recent steps
        if !self.recent_steps.is_empty() {
            let steps_summary: Vec<String> = self
                .recent_steps
                .iter()
                .map(|s| {
                    let status = if s.success { "✓" } else { "✗" };
                    format!("{} {}: {}", status, s.action_type, s.description)
                })
                .collect();
            parts.push(format!("Recent: {}", steps_summary.join("; ")));
        }

        let summary = parts.join(" | ");

        // Truncate if too long
        if summary.len() > max_len {
            format!("{}...", &summary[..max_len - 3])
        } else {
            summary
        }
    }

    /// Create a prompt-ready context string
    ///
    /// Formats the context information for inclusion in an LLM prompt.
    pub fn to_prompt(&self) -> String {
        let mut lines = Vec::new();

        if let Some(ref request) = self.original_request {
            lines.push(format!("Original Request: {}", request));
        }
        if let Some(ref dir) = self.working_directory {
            lines.push(format!("Working Directory: {}", dir));
        }
        if let Some(ref app) = self.current_app {
            lines.push(format!("Current App: {}", app));
        }
        if let Some(ref history) = self.history_summary {
            lines.push(format!("Progress: {}", history));
        }
        if !self.recent_steps.is_empty() {
            let steps: Vec<String> = self
                .recent_steps
                .iter()
                .take(5) // Limit to 5 most recent
                .map(|s| {
                    let status = if s.success { "done" } else { "failed" };
                    format!("- {} ({}): {}", s.action_type, status, s.description)
                })
                .collect();
            lines.push(format!("Recent Steps:\n{}", steps.join("\n")));
        }

        lines.join("\n")
    }
}

impl SubAgentRequest {
    /// Create a new sub-agent request
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            prompt: prompt.into(),
            target: None,
            context: HashMap::new(),
            max_iterations: None,
            parent_session_id: None,
            execution_context: None,
        }
    }

    /// Set the target (e.g., MCP server name)
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Add context
    pub fn with_context(mut self, key: impl Into<String>, value: Value) -> Self {
        self.context.insert(key.into(), value);
        self
    }

    /// Set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set parent session ID
    pub fn with_parent_session(mut self, session_id: impl Into<String>) -> Self {
        self.parent_session_id = Some(session_id.into());
        self
    }

    /// Set execution context from main agent
    pub fn with_execution_context(mut self, context: ExecutionContextInfo) -> Self {
        self.execution_context = Some(context);
        self
    }

    /// Create a sub-agent request from parent execution context
    ///
    /// This extracts relevant information from the parent's ExecutionContext
    /// and populates the ExecutionContextInfo for the sub-agent.
    ///
    /// # Arguments
    ///
    /// * `prompt` - The task/prompt for the sub-agent
    /// * `parent_session_id` - The parent session's ID for tracking
    /// * `working_directory` - Current working directory (if applicable)
    /// * `original_request` - The original user request
    /// * `history_summary` - Summary of what has been done so far
    /// * `recent_steps` - Recent steps for context
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = SubAgentRequest::from_parent_context(
    ///     "Search for Rust files",
    ///     "session-123",
    ///     Some("/project"),
    ///     Some("Help me refactor the code"),
    ///     Some("Found 10 files, analyzed 3"),
    ///     vec![
    ///         StepContextInfo::new("glob", "Found *.rs files", true),
    ///         StepContextInfo::new("read", "Read main.rs", true),
    ///     ],
    /// );
    /// ```
    pub fn from_parent_context(
        prompt: impl Into<String>,
        parent_session_id: impl Into<String>,
        working_directory: Option<String>,
        original_request: Option<String>,
        history_summary: Option<String>,
        recent_steps: Vec<StepContextInfo>,
    ) -> Self {
        let mut exec_context = ExecutionContextInfo::new();

        if let Some(dir) = working_directory {
            exec_context = exec_context.with_working_directory(dir);
        }
        if let Some(req) = original_request {
            exec_context = exec_context.with_original_request(req);
        }
        if let Some(summary) = history_summary {
            exec_context = exec_context.with_history_summary(summary);
        }
        for step in recent_steps {
            exec_context = exec_context.with_step(step);
        }

        Self::new(prompt)
            .with_parent_session(parent_session_id)
            .with_execution_context(exec_context)
    }
}

/// Result from a sub-agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// Request ID this result corresponds to
    pub request_id: String,
    /// Whether the execution was successful
    pub success: bool,
    /// Summary of what was accomplished
    pub summary: String,
    /// Detailed output (optional)
    pub output: Option<Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Number of iterations used
    pub iterations_used: u32,
    /// Tools that were called
    pub tools_called: Vec<ToolCallRecord>,
    /// Artifacts produced (file paths, URLs, etc.)
    pub artifacts: Vec<Artifact>,
}

impl SubAgentResult {
    /// Create a successful result
    pub fn success(request_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            success: true,
            summary: summary.into(),
            output: None,
            error: None,
            iterations_used: 0,
            tools_called: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    /// Create a failed result
    pub fn failure(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            success: false,
            summary: String::new(),
            output: None,
            error: Some(error.into()),
            iterations_used: 0,
            tools_called: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    /// Add output
    pub fn with_output(mut self, output: Value) -> Self {
        self.output = Some(output);
        self
    }

    /// Add iterations used
    pub fn with_iterations(mut self, iterations: u32) -> Self {
        self.iterations_used = iterations;
        self
    }

    /// Add tool calls
    pub fn with_tools_called(mut self, tools: Vec<ToolCallRecord>) -> Self {
        self.tools_called = tools;
        self
    }

    /// Add artifacts
    pub fn with_artifacts(mut self, artifacts: Vec<Artifact>) -> Self {
        self.artifacts = artifacts;
        self
    }
}

/// Record of a tool call made by the sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Tool name
    pub name: String,
    /// Tool arguments
    pub arguments: Value,
    /// Whether the call succeeded
    pub success: bool,
    /// Brief result summary
    pub result_summary: String,
}

/// Artifact produced by the sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact type (file, url, data, etc.)
    pub artifact_type: String,
    /// Path or identifier
    pub path: String,
    /// MIME type if applicable
    pub mime_type: Option<String>,
    /// Additional metadata
    pub metadata: HashMap<String, Value>,
}

impl Artifact {
    /// Create a file artifact
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            artifact_type: "file".to_string(),
            path: path.into(),
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a URL artifact
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            artifact_type: "url".to_string(),
            path: url.into(),
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Set MIME type
    pub fn with_mime_type(mut self, mime: impl Into<String>) -> Self {
        self.mime_type = Some(mime.into());
        self
    }
}

/// The core sub-agent trait
///
/// All specialized sub-agents must implement this trait.
#[async_trait]
pub trait SubAgent: Send + Sync {
    /// Get the sub-agent's unique ID
    fn id(&self) -> &str;

    /// Get the sub-agent's display name
    fn name(&self) -> &str;

    /// Get the sub-agent's description
    fn description(&self) -> &str;

    /// Get the capabilities this sub-agent provides
    fn capabilities(&self) -> Vec<SubAgentCapability>;

    /// Check if this sub-agent can handle the given request
    fn can_handle(&self, request: &SubAgentRequest) -> bool;

    /// Execute the request
    ///
    /// This is the main entry point for sub-agent execution.
    async fn execute(&self, request: SubAgentRequest) -> Result<SubAgentResult>;

    /// Get available tools/actions for this sub-agent
    fn available_actions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Check if a specific action is available
    fn has_action(&self, action: &str) -> bool {
        self.available_actions().iter().any(|a| a == action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_request_builder() {
        let request = SubAgentRequest::new("List all PRs")
            .with_target("github")
            .with_max_iterations(5)
            .with_context("repo", Value::String("owner/repo".to_string()));

        assert_eq!(request.prompt, "List all PRs");
        assert_eq!(request.target, Some("github".to_string()));
        assert_eq!(request.max_iterations, Some(5));
        assert!(request.context.contains_key("repo"));
    }

    #[test]
    fn test_sub_agent_result_success() {
        let result = SubAgentResult::success("req-1", "Completed successfully")
            .with_iterations(3)
            .with_artifacts(vec![Artifact::file("/tmp/output.txt")]);

        assert!(result.success);
        assert_eq!(result.summary, "Completed successfully");
        assert_eq!(result.iterations_used, 3);
        assert_eq!(result.artifacts.len(), 1);
    }

    #[test]
    fn test_sub_agent_result_failure() {
        let result = SubAgentResult::failure("req-2", "Connection timeout");

        assert!(!result.success);
        assert_eq!(result.error, Some("Connection timeout".to_string()));
    }

    #[test]
    fn test_artifact_types() {
        let file = Artifact::file("/path/to/file.txt").with_mime_type("text/plain");
        assert_eq!(file.artifact_type, "file");
        assert_eq!(file.mime_type, Some("text/plain".to_string()));

        let url = Artifact::url("https://example.com/image.png");
        assert_eq!(url.artifact_type, "url");
    }

    #[test]
    fn test_step_context_info_creation() {
        let step = StepContextInfo::new("glob", "Found 10 files", true);
        assert_eq!(step.action_type, "glob");
        assert_eq!(step.description, "Found 10 files");
        assert!(step.success);

        let success_step = StepContextInfo::success("read", "Read config.toml");
        assert!(success_step.success);

        let failed_step = StepContextInfo::failure("write", "Permission denied");
        assert!(!failed_step.success);
    }

    #[test]
    fn test_execution_context_info_builder() {
        let ctx = ExecutionContextInfo::new()
            .with_working_directory("/project")
            .with_current_app("VSCode")
            .with_original_request("Help me refactor")
            .with_history_summary("Found 5 files")
            .with_step(StepContextInfo::success("glob", "Listed files"))
            .with_metadata("key1", "value1");

        assert_eq!(ctx.working_directory, Some("/project".to_string()));
        assert_eq!(ctx.current_app, Some("VSCode".to_string()));
        assert_eq!(ctx.original_request, Some("Help me refactor".to_string()));
        assert_eq!(ctx.history_summary, Some("Found 5 files".to_string()));
        assert_eq!(ctx.recent_steps.len(), 1);
        assert_eq!(ctx.metadata.get("key1"), Some(&"value1".to_string()));
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_execution_context_info_is_empty() {
        let empty = ExecutionContextInfo::new();
        assert!(empty.is_empty());

        let not_empty = ExecutionContextInfo::new().with_working_directory("/tmp");
        assert!(!not_empty.is_empty());
    }

    #[test]
    fn test_execution_context_info_build_summary() {
        let ctx = ExecutionContextInfo::new()
            .with_history_summary("Initial setup complete")
            .with_step(StepContextInfo::success("glob", "Found files"))
            .with_step(StepContextInfo::failure("read", "File not found"));

        let summary = ctx.build_summary(None);
        assert!(summary.contains("Initial setup complete"));
        assert!(summary.contains("✓ glob"));
        assert!(summary.contains("✗ read"));
    }

    #[test]
    fn test_execution_context_info_build_summary_truncation() {
        let ctx = ExecutionContextInfo::new()
            .with_history_summary("A very long history summary that goes on and on...");

        let summary = ctx.build_summary(Some(30));
        assert_eq!(summary.len(), 30);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn test_execution_context_info_to_prompt() {
        let ctx = ExecutionContextInfo::new()
            .with_original_request("Refactor the code")
            .with_working_directory("/project")
            .with_history_summary("Found 10 files")
            .with_step(StepContextInfo::success("glob", "Listed *.rs"));

        let prompt = ctx.to_prompt();
        assert!(prompt.contains("Original Request: Refactor the code"));
        assert!(prompt.contains("Working Directory: /project"));
        assert!(prompt.contains("Progress: Found 10 files"));
        assert!(prompt.contains("glob (done)"));
    }

    #[test]
    fn test_sub_agent_request_from_parent_context() {
        let request = SubAgentRequest::from_parent_context(
            "Search for Rust files",
            "parent-session-123",
            Some("/project".to_string()),
            Some("Help me refactor".to_string()),
            Some("Found 5 files".to_string()),
            vec![
                StepContextInfo::success("glob", "Listed files"),
                StepContextInfo::success("read", "Read main.rs"),
            ],
        );

        assert_eq!(request.prompt, "Search for Rust files");
        assert_eq!(request.parent_session_id, Some("parent-session-123".to_string()));
        assert!(request.execution_context.is_some());

        let ctx = request.execution_context.unwrap();
        assert_eq!(ctx.working_directory, Some("/project".to_string()));
        assert_eq!(ctx.original_request, Some("Help me refactor".to_string()));
        assert_eq!(ctx.history_summary, Some("Found 5 files".to_string()));
        assert_eq!(ctx.recent_steps.len(), 2);
    }

    #[test]
    fn test_sub_agent_request_from_parent_context_minimal() {
        let request = SubAgentRequest::from_parent_context(
            "Quick task",
            "session-1",
            None,
            None,
            None,
            vec![],
        );

        assert_eq!(request.prompt, "Quick task");
        assert!(request.execution_context.is_some());

        let ctx = request.execution_context.unwrap();
        assert!(ctx.is_empty());
    }
}

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
}

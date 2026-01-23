//! Delegate Tool
//!
//! A tool that allows the main agent to delegate tasks to specialized sub-agents.
//! This implements the rig-core Tool trait for integration with the agent loop.

use std::sync::Arc;

use rig::tool::{Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::info;

use super::dispatcher::SubAgentDispatcher;
use super::traits::{SubAgentRequest, ExecutionContextInfo};
use crate::error::Result;

/// Arguments for the delegate tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateArgs {
    /// The task/prompt to delegate
    pub prompt: String,
    /// Target agent type (optional): "mcp", "skill", or specific agent ID
    #[serde(default)]
    pub agent: Option<String>,
    /// Target for the sub-agent (e.g., MCP server name, skill ID)
    #[serde(default)]
    pub target: Option<String>,
    /// Additional context to pass to the sub-agent
    #[serde(default)]
    pub context: Option<Value>,
    /// Maximum iterations for the sub-agent
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Execution context from main agent (working directory, history, etc.)
    #[serde(default)]
    pub execution_context: Option<ExecutionContextInfo>,
}

/// Result from the delegate tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateResult {
    /// Whether the delegation was successful
    pub success: bool,
    /// Summary of the result
    pub summary: String,
    /// Agent that handled the request
    pub agent_id: String,
    /// Detailed output (if any)
    pub output: Option<Value>,
    /// Artifacts produced (file paths, URLs, etc.)
    pub artifacts: Vec<ArtifactInfo>,
    /// Tools called by the sub-agent
    pub tools_called: Vec<ToolCallInfo>,
    /// Number of iterations the sub-agent used
    pub iterations_used: u32,
    /// Error message (if failed)
    pub error: Option<String>,
}

/// Information about an artifact produced by sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    /// Artifact type (file, url, data)
    pub artifact_type: String,
    /// Path or identifier
    pub path: String,
    /// MIME type if known
    pub mime_type: Option<String>,
}

/// Information about a tool call made by sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Tool name
    pub name: String,
    /// Whether the call succeeded
    pub success: bool,
    /// Brief result summary
    pub result_summary: String,
}

/// Delegate Tool for the main agent
///
/// This tool allows the main agent to delegate tasks to specialized sub-agents:
/// - MCP Agent: For discovering and understanding MCP tools
/// - Skill Agent: For discovering and understanding skill workflows
///
/// # Usage
///
/// ```json
/// {
///   "prompt": "List all open pull requests",
///   "agent": "mcp",
///   "target": "github",
///   "context": {"repo": "owner/repo"}
/// }
/// ```
pub struct DelegateTool {
    /// Sub-agent dispatcher
    dispatcher: Arc<RwLock<SubAgentDispatcher>>,
    /// Current session ID (for tracking)
    session_id: Option<String>,
    /// Default execution context to use if not provided in args
    default_execution_context: Option<ExecutionContextInfo>,
}

impl DelegateTool {
    /// Create a new delegate tool
    pub fn new(dispatcher: Arc<RwLock<SubAgentDispatcher>>) -> Self {
        Self {
            dispatcher,
            session_id: None,
            default_execution_context: None,
        }
    }

    /// Set the session ID for tracking
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set default execution context (used when args don't provide one)
    pub fn with_execution_context(mut self, context: ExecutionContextInfo) -> Self {
        self.default_execution_context = Some(context);
        self
    }

    /// Execute delegation
    async fn delegate(&self, args: DelegateArgs) -> Result<DelegateResult> {
        info!(
            "Delegating task: {}",
            args.prompt.chars().take(50).collect::<String>()
        );

        // Build the sub-agent request
        let mut request = SubAgentRequest::new(&args.prompt);

        if let Some(ref target) = args.target {
            request = request.with_target(target);
        }

        if let Some(ref context) = args.context {
            if let Some(obj) = context.as_object() {
                for (key, value) in obj {
                    request = request.with_context(key, value.clone());
                }
            }
        }

        if let Some(max_iter) = args.max_iterations {
            request = request.with_max_iterations(max_iter);
        }

        if let Some(ref session_id) = self.session_id {
            request = request.with_parent_session(session_id);
        }

        // Add agent hint to context if specified
        if let Some(ref agent) = args.agent {
            request = request.with_context("agent_type", Value::String(agent.clone()));
        }

        // Pass execution context (prefer args context, fall back to default)
        if let Some(exec_ctx) = args.execution_context.or_else(|| self.default_execution_context.clone()) {
            request = request.with_execution_context(exec_ctx);
        }

        // Dispatch the request
        let dispatcher = self.dispatcher.read().await;
        let result = dispatcher.dispatch(request).await?;

        // Convert artifacts to ArtifactInfo
        let artifacts: Vec<ArtifactInfo> = result.artifacts.iter().map(|a| ArtifactInfo {
            artifact_type: a.artifact_type.clone(),
            path: a.path.clone(),
            mime_type: a.mime_type.clone(),
        }).collect();

        // Convert tool calls to ToolCallInfo
        let tools_called: Vec<ToolCallInfo> = result.tools_called.iter().map(|tc| ToolCallInfo {
            name: tc.name.clone(),
            success: tc.success,
            result_summary: tc.result_summary.clone(),
        }).collect();

        Ok(DelegateResult {
            success: result.success,
            summary: result.summary,
            agent_id: args.agent.unwrap_or_else(|| "auto".to_string()),
            output: result.output,
            artifacts,
            tools_called,
            iterations_used: result.iterations_used,
            error: result.error,
        })
    }
}

// Implement rig-core Tool trait
impl Tool for DelegateTool {
    const NAME: &'static str = "delegate";

    type Error = ToolError;
    type Args = DelegateArgs;
    type Output = DelegateResult;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Delegate a task to a specialized sub-agent. Use this when:\n\
                - You need to discover MCP tools from external servers (agent: \"mcp\")\n\
                - You need to find available skill workflows (agent: \"skill\")\n\
                - A task requires specialized tool discovery\n\n\
                The sub-agent will analyze available tools and provide recommendations."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task to delegate to the sub-agent"
                    },
                    "agent": {
                        "type": "string",
                        "enum": ["mcp", "skill"],
                        "description": "The type of sub-agent to use: 'mcp' for MCP tools, 'skill' for skill workflows"
                    },
                    "target": {
                        "type": "string",
                        "description": "Target for the sub-agent (e.g., MCP server name like 'github', or skill ID)"
                    },
                    "context": {
                        "type": "object",
                        "description": "Additional context to pass to the sub-agent"
                    },
                    "max_iterations": {
                        "type": "integer",
                        "description": "Maximum iterations for the sub-agent (default: 10)"
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        self.delegate(args)
            .await
            .map_err(|e| ToolError::ToolCallError(e.to_string().into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolRegistry;

    #[tokio::test]
    async fn test_delegate_tool_creation() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::with_defaults(registry)));
        let tool = DelegateTool::new(dispatcher);

        assert_eq!(DelegateTool::NAME, "delegate");
        assert!(tool.session_id.is_none());
    }

    #[tokio::test]
    async fn test_delegate_tool_definition() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::with_defaults(registry)));
        let tool = DelegateTool::new(dispatcher);

        let definition = Tool::definition(&tool, "test".to_string()).await;
        assert_eq!(definition.name, "delegate");
        assert!(definition.description.contains("sub-agent"));
    }

    #[tokio::test]
    async fn test_delegate_to_mcp_agent() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::with_defaults(registry)));
        let tool = DelegateTool::new(dispatcher);

        let args = DelegateArgs {
            prompt: "List PRs".to_string(),
            agent: Some("mcp".to_string()),
            target: Some("github".to_string()),
            context: None,
            max_iterations: None,
            execution_context: None,
        };

        let result = tool.delegate(args).await.unwrap();
        // Will succeed with info about available tools
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_delegate_to_skill_agent() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::with_defaults(registry)));
        let tool = DelegateTool::new(dispatcher);

        let args = DelegateArgs {
            prompt: "Execute workflow".to_string(),
            agent: Some("skill".to_string()),
            target: None,
            context: None,
            max_iterations: None,
            execution_context: None,
        };

        let result = tool.delegate(args).await.unwrap();
        // Will succeed with info about available skills
        assert!(result.success);
    }

    #[test]
    fn test_delegate_args_parsing() {
        let json = r#"{
            "prompt": "List PRs",
            "agent": "mcp",
            "target": "github",
            "context": {"repo": "owner/repo"},
            "max_iterations": 5
        }"#;

        let args: DelegateArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.prompt, "List PRs");
        assert_eq!(args.agent, Some("mcp".to_string()));
        assert_eq!(args.target, Some("github".to_string()));
        assert_eq!(args.max_iterations, Some(5));
        assert!(args.execution_context.is_none());
    }

    #[tokio::test]
    async fn test_delegate_with_execution_context() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let dispatcher = Arc::new(RwLock::new(SubAgentDispatcher::with_defaults(registry)));

        // Create tool with default execution context
        let exec_ctx = ExecutionContextInfo::new()
            .with_working_directory("/Users/test/project")
            .with_original_request("User wants to list MCP tools")
            .with_history_summary("Previous steps: searched files, read config");

        let tool = DelegateTool::new(dispatcher)
            .with_session_id("session-123")
            .with_execution_context(exec_ctx);

        let args = DelegateArgs {
            prompt: "List available tools".to_string(),
            agent: Some("mcp".to_string()),
            target: None,
            context: None,
            max_iterations: None,
            execution_context: None, // Will use default from tool
        };

        let result = tool.delegate(args).await.unwrap();
        assert!(result.success);
    }

    #[test]
    fn test_execution_context_builder() {
        let ctx = ExecutionContextInfo::new()
            .with_working_directory("/home/user")
            .with_current_app("VSCode")
            .with_window_title("main.rs - project")
            .with_original_request("Help me with this code")
            .with_history_summary("Analyzed file structure")
            .with_metadata("theme", "dark");

        assert_eq!(ctx.working_directory, Some("/home/user".to_string()));
        assert_eq!(ctx.current_app, Some("VSCode".to_string()));
        assert_eq!(ctx.window_title, Some("main.rs - project".to_string()));
        assert_eq!(ctx.original_request, Some("Help me with this code".to_string()));
        assert!(!ctx.is_empty());
    }
}

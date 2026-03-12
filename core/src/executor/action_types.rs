//! Local action types for the executor module
//!
//! These types were previously in agent_loop but have been moved here
//! since they are only used by the executor subsystem.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool call request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// A question group for multi-group user interaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionGroup {
    pub label: String,
    pub options: Vec<String>,
}

/// Rich user response with structured feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichUserResponse {
    pub response: String,
    #[serde(default)]
    pub metadata: Value,
}

impl RichUserResponse {
    pub fn to_llm_feedback(&self) -> String {
        self.response.clone()
    }
}

/// An action that the agent decides to take
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Execute one or more tool calls
    ToolCalls { calls: Vec<ToolCallRequest> },
    /// Ask the user a question
    UserInteraction {
        question: String,
        options: Option<Vec<String>>,
    },
    /// Multi-group user interaction
    UserInteractionMultigroup {
        question: String,
        groups: Vec<QuestionGroup>,
    },
    /// Rich user interaction
    UserInteractionRich {
        question: String,
        options: Option<Vec<String>>,
    },
    /// Task completed successfully
    Completion { summary: String },
    /// Task failed
    Failure { reason: String },
}

impl Action {
    /// Get the action type string
    pub fn action_type(&self) -> &str {
        match self {
            Action::ToolCalls { calls } => {
                if let Some(req) = calls.first() {
                    // Return tool name for single tool call
                    return &req.tool_name;
                }
                "tool_calls"
            }
            Action::UserInteraction { .. } => "ask_user",
            Action::UserInteractionMultigroup { .. } => "ask_user_multigroup",
            Action::UserInteractionRich { .. } => "ask_user_rich",
            Action::Completion { .. } => "completion",
            Action::Failure { .. } => "failure",
        }
    }

    /// Get a summary of arguments
    pub fn args_summary(&self) -> String {
        match self {
            Action::ToolCalls { calls } => {
                if let Some(req) = calls.first() {
                    serde_json::to_string(&req.arguments).unwrap_or_default()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        }
    }
}

/// Result of a single tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SingleToolResult {
    Success { output: Value, duration_ms: u64 },
    Error { error: String, retryable: bool },
}

/// Result of a tool call (wraps SingleToolResult with metadata)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: String,
    pub tool_name: String,
    pub result: SingleToolResult,
}

/// The result of executing an action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionResult {
    /// Tool execution results
    ToolResults { results: Vec<ToolCallResult> },
    /// User provided a response
    UserResponse { response: String },
    /// Rich user response
    UserResponseRich { response: RichUserResponse },
    /// Task completed
    Completed,
    /// Task failed
    Failed,
}

impl ActionResult {
    /// Check if the result indicates success
    pub fn is_success(&self) -> bool {
        match self {
            ActionResult::ToolResults { results } => {
                results.iter().all(|r| matches!(r.result, SingleToolResult::Success { .. }))
            }
            ActionResult::UserResponse { .. } | ActionResult::UserResponseRich { .. } => true,
            ActionResult::Completed => true,
            ActionResult::Failed => false,
        }
    }

    /// Check if the result is a non-retryable error
    pub fn is_non_retryable_error(&self) -> bool {
        match self {
            ActionResult::ToolResults { results } => {
                results.iter().any(|r| matches!(r.result, SingleToolResult::Error { retryable: false, .. }))
            }
            ActionResult::Failed => true,
            _ => false,
        }
    }

    /// Get the first tool output value
    pub fn first_tool_output(&self) -> Option<Value> {
        match self {
            ActionResult::ToolResults { results } => {
                results.first().and_then(|r| match &r.result {
                    SingleToolResult::Success { output, .. } => Some(output.clone()),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Get the first tool error message
    pub fn first_tool_error(&self) -> Option<String> {
        match self {
            ActionResult::ToolResults { results } => {
                results.first().and_then(|r| match &r.result {
                    SingleToolResult::Error { error, .. } => Some(error.clone()),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Get a summary string
    pub fn summary(&self) -> String {
        match self {
            ActionResult::ToolResults { results } => {
                if let Some(r) = results.first() {
                    match &r.result {
                        SingleToolResult::Success { output, .. } => {
                            serde_json::to_string(output).unwrap_or_else(|_| "success".to_string())
                        }
                        SingleToolResult::Error { error, .. } => format!("Error: {}", error),
                    }
                } else {
                    "No results".to_string()
                }
            }
            ActionResult::UserResponse { response } => response.clone(),
            ActionResult::UserResponseRich { response } => response.to_llm_feedback(),
            ActionResult::Completed => "Completed".to_string(),
            ActionResult::Failed => "Failed".to_string(),
        }
    }
}

/// Trait for executing actions
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    /// Execute an action and return the result
    async fn execute(
        &self,
        action: &Action,
        identity: &aleph_protocol::IdentityContext,
    ) -> ActionResult;
}

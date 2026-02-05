//! Decision types for Agent Loop
//!
//! This module defines the core decision types that represent
//! LLM decisions, actions, and their results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::answer::UserAnswer;
use super::question::QuestionKind;

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// LLM's decision for the next action
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    /// Execute a tool
    UseTool {
        tool_name: String,
        arguments: Value,
    },
    /// Request user input
    AskUser {
        question: String,
        #[serde(default)]
        options: Option<Vec<String>>,
    },
    /// Request multi-group user input
    AskUserMultigroup {
        question: String,
        groups: Vec<QuestionGroup>,
    },
    /// Request rich user input with structured question type
    AskUserRich {
        question: String,
        kind: QuestionKind,
        #[serde(default)]
        question_id: Option<String>,
    },
    /// Task completed successfully
    Complete {
        summary: String,
    },
    /// Task failed
    Fail {
        reason: String,
    },
    /// Silent response - nothing to report
    Silent,
    /// Heartbeat acknowledgment - background task alive
    HeartbeatOk,
}

/// Question group for multi-group clarifications
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionGroup {
    pub id: String,
    pub prompt: String,
    pub options: Vec<String>,
}

impl Decision {
    /// Check if this decision is terminal (ends the loop)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Decision::Complete { .. }
                | Decision::Fail { .. }
                | Decision::Silent
                | Decision::HeartbeatOk
        )
    }

    /// Get decision type as string
    pub fn decision_type(&self) -> &'static str {
        match self {
            Decision::UseTool { .. } => "tool",
            Decision::AskUser { .. } => "ask_user",
            Decision::AskUserMultigroup { .. } => "ask_user_multigroup",
            Decision::AskUserRich { .. } => "ask_user_rich",
            Decision::Complete { .. } => "complete",
            Decision::Fail { .. } => "fail",
            Decision::Silent => "silent",
            Decision::HeartbeatOk => "heartbeat_ok",
        }
    }
}

/// Action to be executed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Tool invocation
    ToolCall {
        tool_name: String,
        arguments: Value,
    },
    /// User interaction request
    UserInteraction {
        question: String,
        #[serde(default)]
        options: Option<Vec<String>>,
    },
    /// Multi-group user interaction request
    UserInteractionMultigroup {
        question: String,
        groups: Vec<QuestionGroup>,
    },
    /// Rich user interaction request
    UserInteractionRich {
        question: String,
        kind: QuestionKind,
        #[serde(default)]
        question_id: Option<String>,
    },
    /// Task completion
    Completion {
        summary: String,
    },
    /// Task failure
    Failure {
        reason: String,
    },
}

impl Action {
    /// Get action type as string
    ///
    /// For tools with an "operation" field in arguments (like file_ops),
    /// the operation is included in the action type to distinguish
    /// different operations (e.g., "tool:file_ops:mkdir" vs "tool:file_ops:write").
    /// This prevents false StuckLoop detection when the same tool is called
    /// with different operations.
    pub fn action_type(&self) -> String {
        match self {
            Action::ToolCall {
                tool_name,
                arguments,
            } => {
                // Check if arguments has an "operation" field
                // Supports both string values ("mkdir") and Value types
                if let Some(operation) = arguments.get("operation") {
                    // Try to get as string directly
                    if let Some(op_str) = operation.as_str() {
                        return format!("tool:{}:{}", tool_name, op_str);
                    }
                    // Log unexpected operation type for debugging
                    tracing::debug!(
                        tool = %tool_name,
                        operation_type = ?operation,
                        "operation field exists but is not a string"
                    );
                }
                format!("tool:{}", tool_name)
            }
            Action::UserInteraction { .. } => "ask_user".to_string(),
            Action::UserInteractionMultigroup { .. } => "ask_user_multigroup".to_string(),
            Action::UserInteractionRich { .. } => "ask_user_rich".to_string(),
            Action::Completion { .. } => "complete".to_string(),
            Action::Failure { .. } => "fail".to_string(),
        }
    }

    /// Get action arguments summary
    pub fn args_summary(&self) -> String {
        match self {
            Action::ToolCall { arguments, .. } => {
                // Truncate long arguments
                let s = arguments.to_string();
                truncate_str(&s, 100)
            }
            Action::UserInteraction { question, .. } => question.clone(),
            Action::UserInteractionMultigroup { question, groups } => {
                format!("{} ({} groups)", question, groups.len())
            }
            Action::UserInteractionRich { question, kind, .. } => {
                format!("{} (type: {:?})", question, std::mem::discriminant(kind))
            }
            Action::Completion { summary } => summary.clone(),
            Action::Failure { reason } => reason.clone(),
        }
    }

    /// Check if this action is terminal
    pub fn is_terminal(&self) -> bool {
        matches!(self, Action::Completion { .. } | Action::Failure { .. })
    }
}

impl From<Decision> for Action {
    fn from(decision: Decision) -> Self {
        match decision {
            Decision::UseTool {
                tool_name,
                arguments,
            } => Action::ToolCall {
                tool_name,
                arguments,
            },
            Decision::AskUser { question, options } => Action::UserInteraction { question, options },
            Decision::AskUserMultigroup { question, groups } => {
                Action::UserInteractionMultigroup { question, groups }
            }
            Decision::AskUserRich { question, kind, question_id } => {
                Action::UserInteractionRich { question, kind, question_id }
            }
            Decision::Complete { summary } => Action::Completion { summary },
            Decision::Fail { reason } => Action::Failure { reason },
            Decision::Silent => Action::Completion { summary: "[silent]".to_string() },
            Decision::HeartbeatOk => Action::Completion { summary: "[heartbeat_ok]".to_string() },
        }
    }
}

/// Result of an action execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionResult {
    /// Tool executed successfully
    ToolSuccess {
        output: Value,
        #[serde(default)]
        duration_ms: u64,
    },
    /// Tool execution failed
    ToolError {
        error: String,
        #[serde(default)]
        retryable: bool,
    },
    /// User provided response
    UserResponse {
        response: String,
    },
    /// User provided structured response
    UserResponseRich {
        response: UserAnswer,
    },
    /// Task completed
    Completed,
    /// Task failed
    Failed,
}

impl ActionResult {
    /// Check if result indicates success
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            ActionResult::ToolSuccess { .. }
                | ActionResult::UserResponse { .. }
                | ActionResult::UserResponseRich { .. }
                | ActionResult::Completed
        )
    }

    /// Check if result is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(self, ActionResult::ToolError { retryable: true, .. })
    }

    /// Get result summary (truncated for display)
    pub fn summary(&self) -> String {
        match self {
            ActionResult::ToolSuccess { output, duration_ms } => {
                let s = output.to_string();
                let truncated = truncate_str(&s, 50);
                format!("Success ({}ms): {}", duration_ms, truncated)
            }
            ActionResult::ToolError { error, retryable } => {
                format!("Error (retryable={}): {}", retryable, error)
            }
            ActionResult::UserResponse { response } => {
                format!("User: {}", response)
            }
            ActionResult::UserResponseRich { response } => {
                format!("User: {}", response.to_llm_feedback())
            }
            ActionResult::Completed => "Completed".to_string(),
            ActionResult::Failed => "Failed".to_string(),
        }
    }

    /// Get full output for LLM context (not truncated)
    ///
    /// This method returns the complete tool output without truncation,
    /// ensuring the LLM has access to full file paths, complete JSON data,
    /// and other information needed for accurate decision making.
    pub fn full_output(&self) -> String {
        match self {
            ActionResult::ToolSuccess { output, .. } => {
                output.to_string()
            }
            ActionResult::ToolError { error, .. } => {
                format!("Error: {}", error)
            }
            ActionResult::UserResponse { response } => {
                format!("User: {}", response)
            }
            ActionResult::UserResponseRich { response } => {
                format!("User: {}", response.to_llm_feedback())
            }
            ActionResult::Completed => "Completed".to_string(),
            ActionResult::Failed => "Failed".to_string(),
        }
    }
}

/// Parsed LLM response containing thinking and decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Optional reasoning/thinking process
    #[serde(default)]
    pub reasoning: Option<String>,
    /// The action decision
    pub action: LlmAction,
}

/// LLM's action in response format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmAction {
    Tool {
        tool_name: String,
        arguments: Value,
    },
    AskUser {
        question: String,
        #[serde(default)]
        options: Option<Vec<String>>,
    },
    AskUserMultigroup {
        question: String,
        groups: Vec<QuestionGroup>,
    },
    AskUserRich {
        question: String,
        kind: QuestionKind,
        #[serde(default)]
        question_id: Option<String>,
    },
    Complete {
        summary: String,
    },
    Fail {
        reason: String,
    },
    /// Silent - no output needed
    Silent,
    /// Heartbeat OK - background task alive
    HeartbeatOk,
}

impl From<LlmAction> for Decision {
    fn from(action: LlmAction) -> Self {
        match action {
            LlmAction::Tool {
                tool_name,
                arguments,
            } => Decision::UseTool {
                tool_name,
                arguments,
            },
            LlmAction::AskUser { question, options } => Decision::AskUser { question, options },
            LlmAction::AskUserMultigroup { question, groups } => {
                Decision::AskUserMultigroup { question, groups }
            }
            LlmAction::AskUserRich { question, kind, question_id } => {
                Decision::AskUserRich { question, kind, question_id }
            }
            LlmAction::Complete { summary } => Decision::Complete { summary },
            LlmAction::Fail { reason } => Decision::Fail { reason },
            LlmAction::Silent => Decision::Silent,
            LlmAction::HeartbeatOk => Decision::HeartbeatOk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_decision_serialization() {
        let decision = Decision::UseTool {
            tool_name: "search".to_string(),
            arguments: json!({"query": "rust tutorial"}),
        };

        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("use_tool"));
        assert!(json.contains("search"));

        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, decision);
    }

    #[test]
    fn test_decision_is_terminal() {
        assert!(!Decision::UseTool {
            tool_name: "test".to_string(),
            arguments: json!({})
        }
        .is_terminal());

        assert!(!Decision::AskUser {
            question: "?".to_string(),
            options: None
        }
        .is_terminal());

        assert!(Decision::Complete {
            summary: "done".to_string()
        }
        .is_terminal());

        assert!(Decision::Fail {
            reason: "error".to_string()
        }
        .is_terminal());
    }

    #[test]
    fn test_action_result_is_success() {
        assert!(ActionResult::ToolSuccess {
            output: json!("ok"),
            duration_ms: 100
        }
        .is_success());

        assert!(!ActionResult::ToolError {
            error: "failed".to_string(),
            retryable: false
        }
        .is_success());

        assert!(ActionResult::UserResponse {
            response: "yes".to_string()
        }
        .is_success());
    }

    #[test]
    fn test_llm_response_parsing() {
        let json = r#"{
            "reasoning": "I need to search for information",
            "action": {
                "type": "tool",
                "tool_name": "web_search",
                "arguments": {"query": "rust async"}
            }
        }"#;

        let response: LlmResponse = serde_json::from_str(json).unwrap();
        assert!(response.reasoning.is_some());

        let decision: Decision = response.action.into();
        assert!(matches!(decision, Decision::UseTool { .. }));
    }

    #[test]
    fn test_action_type_with_operation() {
        // Tool without operation field
        let action = Action::ToolCall {
            tool_name: "search".to_string(),
            arguments: json!({"query": "rust tutorial"}),
        };
        assert_eq!(action.action_type(), "tool:search");

        // Tool with operation field (like file_ops)
        let action_with_op = Action::ToolCall {
            tool_name: "file_ops".to_string(),
            arguments: json!({"operation": "mkdir", "path": "/tmp/test"}),
        };
        assert_eq!(action_with_op.action_type(), "tool:file_ops:mkdir");

        // Different operation on same tool
        let action_write = Action::ToolCall {
            tool_name: "file_ops".to_string(),
            arguments: json!({"operation": "write", "path": "/tmp/test.txt", "content": "hello"}),
        };
        assert_eq!(action_write.action_type(), "tool:file_ops:write");

        // Non-tool actions
        assert_eq!(
            Action::UserInteraction {
                question: "test?".to_string(),
                options: None
            }
            .action_type(),
            "ask_user"
        );
        assert_eq!(
            Action::Completion {
                summary: "done".to_string()
            }
            .action_type(),
            "complete"
        );
        assert_eq!(
            Action::Failure {
                reason: "error".to_string()
            }
            .action_type(),
            "fail"
        );
    }

    #[test]
    fn test_ask_user_rich_serialization() {
        use super::super::question::{ChoiceOption, QuestionKind};

        let decision = Decision::AskUserRich {
            question: "Choose an option".to_string(),
            kind: QuestionKind::SingleChoice {
                choices: vec![
                    ChoiceOption::new("Option A"),
                    ChoiceOption::new("Option B"),
                ],
                default_index: Some(0),
            },
            question_id: None,
        };

        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("ask_user_rich"));
        assert!(json.contains("single_choice"));

        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, Decision::AskUserRich { .. }));
    }

    #[test]
    fn test_user_response_rich() {
        use super::super::answer::UserAnswer;

        let result = ActionResult::UserResponseRich {
            response: UserAnswer::SingleChoice {
                selected_index: 0,
                selected_label: "Option A".to_string(),
            },
        };

        assert!(result.is_success());
        assert!(result.summary().contains("Option A"));
    }
}

//! Pipeline Result Types
//!
//! Result types for the intent routing pipeline:
//!
//! - `PipelineResult`: Final result of pipeline processing
//! - `ResumeResult`: Result of resuming from clarification
//! - `ClarificationError`: Errors during clarification flow
//! - `ClarificationRequest`: Request sent to UI for clarification

use crate::routing::{AggregatedIntent, RoutingContext};
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Pipeline Result
// =============================================================================

/// Result of pipeline processing
#[derive(Debug, Clone)]
pub enum PipelineResult {
    /// Tool was executed successfully
    Executed {
        /// Tool name that was executed
        tool_name: String,
        /// Result content
        content: String,
        /// Parameters used
        parameters: serde_json::Value,
    },

    /// Waiting for user clarification
    PendingClarification(ClarificationRequest),

    /// User cancelled the operation
    Cancelled {
        /// Reason for cancellation
        reason: String,
    },

    /// No tool matched - fall back to general chat
    GeneralChat {
        /// Input that was processed
        input: String,
    },

    /// Pipeline was skipped (e.g., disabled)
    Skipped {
        /// Reason for skipping
        reason: String,
    },
}

impl PipelineResult {
    /// Create an executed result
    pub fn executed(
        tool_name: impl Into<String>,
        content: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self::Executed {
            tool_name: tool_name.into(),
            content: content.into(),
            parameters,
        }
    }

    /// Create a cancelled result
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled {
            reason: reason.into(),
        }
    }

    /// Create a general chat result
    pub fn general_chat(input: impl Into<String>) -> Self {
        Self::GeneralChat {
            input: input.into(),
        }
    }

    /// Create a skipped result
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    /// Check if this is a successful execution
    pub fn is_executed(&self) -> bool {
        matches!(self, Self::Executed { .. })
    }

    /// Check if this needs user input
    pub fn needs_user_input(&self) -> bool {
        matches!(self, Self::PendingClarification(_))
    }

    /// Check if this is a general chat fallback
    pub fn is_general_chat(&self) -> bool {
        matches!(self, Self::GeneralChat { .. })
    }

    /// Get the executed content if applicable
    pub fn get_content(&self) -> Option<&str> {
        match self {
            Self::Executed { content, .. } => Some(content),
            _ => None,
        }
    }
}

impl fmt::Display for PipelineResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Executed { tool_name, .. } => write!(f, "Executed tool: {}", tool_name),
            Self::PendingClarification(req) => {
                write!(f, "Pending clarification: {}", req.prompt)
            }
            Self::Cancelled { reason } => write!(f, "Cancelled: {}", reason),
            Self::GeneralChat { .. } => write!(f, "General chat fallback"),
            Self::Skipped { reason } => write!(f, "Skipped: {}", reason),
        }
    }
}

// =============================================================================
// Clarification Request
// =============================================================================

/// Request sent to UI for user clarification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationRequest {
    /// Unique session ID for this clarification
    pub session_id: String,

    /// Prompt to display to the user
    pub prompt: String,

    /// Suggested values (if any)
    pub suggestions: Vec<String>,

    /// Input type for the clarification
    pub input_type: ClarificationInputType,

    /// Parameter name being clarified
    pub param_name: String,

    /// Tool name that needs clarification
    pub tool_name: Option<String>,
}

impl ClarificationRequest {
    /// Create a new clarification request
    pub fn new(session_id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            prompt: prompt.into(),
            suggestions: Vec::new(),
            input_type: ClarificationInputType::Text,
            param_name: String::new(),
            tool_name: None,
        }
    }

    /// Builder: set suggestions
    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.input_type = if suggestions.is_empty() {
            ClarificationInputType::Text
        } else {
            ClarificationInputType::Select
        };
        self.suggestions = suggestions;
        self
    }

    /// Builder: set parameter name
    pub fn with_param_name(mut self, name: impl Into<String>) -> Self {
        self.param_name = name.into();
        self
    }

    /// Builder: set tool name
    pub fn with_tool_name(mut self, name: impl Into<String>) -> Self {
        self.tool_name = Some(name.into());
        self
    }

    /// Builder: set input type
    pub fn with_input_type(mut self, input_type: ClarificationInputType) -> Self {
        self.input_type = input_type;
        self
    }
}

/// Input type for clarification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationInputType {
    /// Free-form text input
    #[default]
    Text,

    /// Select from suggestions
    Select,

    /// Yes/No confirmation
    Confirm,
}

// =============================================================================
// Resume Result
// =============================================================================

/// Result of resuming from clarification
#[derive(Debug, Clone)]
pub struct ResumeResult {
    /// Updated routing context with clarified parameters
    pub context: RoutingContext,

    /// Updated intent with new parameters
    pub intent: AggregatedIntent,
}

impl ResumeResult {
    /// Create a new resume result
    pub fn new(context: RoutingContext, intent: AggregatedIntent) -> Self {
        Self { context, intent }
    }

    /// Check if all parameters are now complete
    pub fn is_complete(&self) -> bool {
        self.intent.parameters_complete
    }
}

// =============================================================================
// Clarification Error
// =============================================================================

/// Errors that can occur during clarification
#[derive(Debug, Clone)]
pub enum ClarificationError {
    /// Session not found
    SessionNotFound,

    /// Session has timed out
    Timeout,

    /// Session was cancelled
    Cancelled,

    /// Invalid user input
    InvalidInput(String),

    /// Internal error
    Internal(String),
}

impl fmt::Display for ClarificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound => write!(f, "Clarification session not found"),
            Self::Timeout => write!(f, "Clarification session timed out"),
            Self::Cancelled => write!(f, "Clarification was cancelled"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Self::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ClarificationError {}

impl From<ClarificationError> for crate::error::AetherError {
    fn from(err: ClarificationError) -> Self {
        match err {
            ClarificationError::SessionNotFound => {
                crate::error::AetherError::other("Clarification session not found")
            }
            ClarificationError::Timeout => crate::error::AetherError::Timeout {
                suggestion: Some("Please try again".to_string()),
            },
            ClarificationError::Cancelled => {
                crate::error::AetherError::other("Clarification cancelled by user")
            }
            ClarificationError::InvalidInput(msg) => {
                crate::error::AetherError::invalid_config(format!("Invalid clarification input: {}", msg))
            }
            ClarificationError::Internal(msg) => crate::error::AetherError::other(msg),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_result_executed() {
        let result = PipelineResult::executed(
            "search",
            "Search results...",
            serde_json::json!({"query": "test"}),
        );

        assert!(result.is_executed());
        assert!(!result.needs_user_input());
        assert_eq!(result.get_content(), Some("Search results..."));
    }

    #[test]
    fn test_pipeline_result_clarification() {
        let request = ClarificationRequest::new("session-123", "请输入城市名称");
        let result = PipelineResult::PendingClarification(request);

        assert!(result.needs_user_input());
        assert!(!result.is_executed());
    }

    #[test]
    fn test_clarification_request() {
        let request = ClarificationRequest::new("session-123", "请选择城市")
            .with_suggestions(vec!["北京".to_string(), "上海".to_string()])
            .with_param_name("location")
            .with_tool_name("search");

        assert_eq!(request.session_id, "session-123");
        assert_eq!(request.suggestions.len(), 2);
        assert_eq!(request.input_type, ClarificationInputType::Select);
        assert_eq!(request.param_name, "location");
        assert_eq!(request.tool_name, Some("search".to_string()));
    }

    #[test]
    fn test_clarification_error_display() {
        let err = ClarificationError::Timeout;
        assert!(err.to_string().contains("timed out"));

        let err = ClarificationError::InvalidInput("bad value".to_string());
        assert!(err.to_string().contains("bad value"));
    }
}

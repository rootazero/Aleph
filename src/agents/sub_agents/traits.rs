//! Sub-Agent Request/Result Types
//!
//! Defines the delegation request/result shapes used by the A2A outbound path
//! (`a2a::sub_agent`). The old builder island (`ExecutionContextInfo`,
//! `StepContextInfo`, `ToolCallRecord`, `Artifact`, `from_parent_context` and
//! their `with_*` builders) was residue of the deleted `SubAgent` trait and
//! had zero production consumers — removed (R10 zero-consumer withdrawal).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request to a sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRequest {
    /// Unique request ID
    pub id: String,
    /// The prompt/task for the sub-agent
    pub prompt: String,
    /// Parent agent identity, stamped onto the emitted `RawMemory(Delegation)`
    /// row so per-agent memory attribution matches the intra-process spawner.
    pub parent_agent_id: Option<String>,
    /// Parent session ID for tracking
    pub parent_session_id: Option<String>,
}

impl SubAgentRequest {
    /// Create a new sub-agent request
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            prompt: prompt.into(),
            parent_agent_id: None,
            parent_session_id: None,
        }
    }

    /// Set the parent agent identity stamped onto delegation memory rows
    pub fn with_parent_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.parent_agent_id = Some(agent_id.into());
        self
    }

    /// Set parent session ID
    pub fn with_parent_session(mut self, session_id: impl Into<String>) -> Self {
        self.parent_session_id = Some(session_id.into());
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
        }
    }

    /// Add output
    #[must_use]
    pub fn with_output(mut self, output: Value) -> Self {
        self.output = Some(output);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_request_builder() {
        let request = SubAgentRequest::new("List all PRs")
            .with_parent_agent("main")
            .with_parent_session("sess-1");

        assert_eq!(request.prompt, "List all PRs");
        assert_eq!(request.parent_agent_id.as_deref(), Some("main"));
        assert_eq!(request.parent_session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn test_sub_agent_result_success() {
        let result = SubAgentResult::success("req-1", "Completed successfully")
            .with_output(Value::String("detail".to_string()));

        assert!(result.success);
        assert_eq!(result.summary, "Completed successfully");
        assert!(result.output.is_some());
    }

    #[test]
    fn test_sub_agent_result_failure() {
        let result = SubAgentResult::failure("req-2", "Connection timeout");

        assert!(!result.success);
        assert_eq!(result.error, Some("Connection timeout".to_string()));
    }
}

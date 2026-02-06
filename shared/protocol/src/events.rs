//! Streaming Event Types
//!
//! Event types for real-time agent feedback via WebSocket.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::thinking::{ConfidenceLevel, ReasoningStepType};

/// Streaming event types for real-time agent feedback
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Agent run has been accepted and started
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: String,
    },

    /// Reasoning/thinking process update
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },

    /// Tool execution started
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },

    /// Tool execution progress update
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },

    /// Tool execution completed
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },

    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        content: String,
        chunk_index: u32,
        is_final: bool,
    },

    /// Agent run completed successfully
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },

    /// Agent run failed with error
    RunError {
        run_id: String,
        seq: u64,
        error: String,
        error_code: Option<String>,
    },

    /// Agent is asking the user a question
    AskUser {
        run_id: String,
        seq: u64,
        question: String,
        options: Vec<String>,
    },

    /// Structured reasoning block with semantic type
    ReasoningBlock {
        run_id: String,
        seq: u64,
        /// Semantic step type (observation, analysis, planning, etc.)
        step_type: ReasoningStepType,
        /// Human-readable label for this block
        label: String,
        /// Content of this reasoning block
        content: String,
        /// Confidence level if determinable
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<ConfidenceLevel>,
        /// Is this the final block before action?
        is_final: bool,
    },

    /// Uncertainty signal from the AI
    UncertaintySignal {
        run_id: String,
        seq: u64,
        /// What the AI is uncertain about
        uncertainty: String,
        /// Suggested action for handling the uncertainty
        suggested_action: UncertaintyAction,
    },
}

/// Suggested action for handling AI uncertainty
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyAction {
    /// Proceed despite uncertainty
    ProceedWithCaution,
    /// Ask user for clarification before proceeding
    AskForClarification,
    /// Use a safer/more conservative approach
    UseSaferApproach,
    /// Stop and wait for user input
    WaitForUser,
}

impl UncertaintyAction {
    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::ProceedWithCaution => "Proceeding with caution despite uncertainty",
            Self::AskForClarification => "Asking user for clarification",
            Self::UseSaferApproach => "Using a safer, more conservative approach",
            Self::WaitForUser => "Waiting for user guidance",
        }
    }
}

impl StreamEvent {
    /// Create a new ReasoningBlock event
    pub fn reasoning_block(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: None,
            is_final,
        }
    }

    /// Create a new ReasoningBlock event with confidence
    pub fn reasoning_block_with_confidence(
        run_id: impl Into<String>,
        seq: u64,
        step_type: ReasoningStepType,
        label: impl Into<String>,
        content: impl Into<String>,
        confidence: ConfidenceLevel,
        is_final: bool,
    ) -> Self {
        Self::ReasoningBlock {
            run_id: run_id.into(),
            seq,
            step_type,
            label: label.into(),
            content: content.into(),
            confidence: Some(confidence),
            is_final,
        }
    }

    /// Create a new UncertaintySignal event
    pub fn uncertainty_signal(
        run_id: impl Into<String>,
        seq: u64,
        uncertainty: impl Into<String>,
        suggested_action: UncertaintyAction,
    ) -> Self {
        Self::UncertaintySignal {
            run_id: run_id.into(),
            seq,
            uncertainty: uncertainty.into(),
            suggested_action,
        }
    }

    /// Get the run_id from any event variant
    pub fn run_id(&self) -> &str {
        match self {
            Self::RunAccepted { run_id, .. }
            | Self::Reasoning { run_id, .. }
            | Self::ToolStart { run_id, .. }
            | Self::ToolUpdate { run_id, .. }
            | Self::ToolEnd { run_id, .. }
            | Self::ResponseChunk { run_id, .. }
            | Self::RunComplete { run_id, .. }
            | Self::RunError { run_id, .. }
            | Self::AskUser { run_id, .. }
            | Self::ReasoningBlock { run_id, .. }
            | Self::UncertaintySignal { run_id, .. } => run_id,
        }
    }

    /// Get the JSON-RPC method name for this event
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::RunAccepted { .. } => "stream.run_accepted",
            Self::Reasoning { .. } => "stream.reasoning",
            Self::ToolStart { .. } => "stream.tool_start",
            Self::ToolUpdate { .. } => "stream.tool_update",
            Self::ToolEnd { .. } => "stream.tool_end",
            Self::ResponseChunk { .. } => "stream.response_chunk",
            Self::RunComplete { .. } => "stream.run_complete",
            Self::RunError { .. } => "stream.run_error",
            Self::AskUser { .. } => "stream.ask_user",
            Self::ReasoningBlock { .. } => "stream.reasoning_block",
            Self::UncertaintySignal { .. } => "stream.uncertainty_signal",
        }
    }
}

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
            metadata: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Summary of a completed agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
}

/// Configuration changed event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChangedEvent {
    /// Changed section path (e.g., "ui.theme")
    pub section: Option<String>,
    /// New config value (full config if section is None)
    pub value: Value,
    /// Change timestamp
    pub timestamp: i64,
}

/// Enhanced summary with tool details and errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedRunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
    #[serde(default)]
    pub tool_summaries: Vec<ToolSummaryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolErrorItem>,
}

/// Tool execution summary item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummaryItem {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,
    pub display_meta: String,
    pub duration_ms: u64,
    pub success: bool,
}

/// Tool error item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolErrorItem {
    pub tool_name: String,
    pub error: String,
    pub tool_id: String,
}

impl EnhancedRunSummary {
    /// Create from basic RunSummary
    pub fn from_basic(basic: &RunSummary, duration_ms: u64) -> Self {
        Self {
            total_tokens: basic.total_tokens,
            tool_calls: basic.tool_calls,
            loops: basic.loops,
            duration_ms,
            final_response: basic.final_response.clone(),
            tool_summaries: Vec::new(),
            reasoning: None,
            errors: Vec::new(),
        }
    }

    /// Add a tool summary
    pub fn add_tool(&mut self, item: ToolSummaryItem) {
        self.tool_summaries.push(item);
    }

    /// Add an error
    pub fn add_error(&mut self, error: ToolErrorItem) {
        self.errors.push(error);
    }

    /// Check if there are any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("output data");
        assert!(result.success);
        assert_eq!(result.output, Some("output data".to_string()));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something went wrong");
        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_stream_event_method_names() {
        let event = StreamEvent::Reasoning {
            run_id: "".to_string(),
            seq: 0,
            content: "".to_string(),
            is_complete: false,
        };
        assert_eq!(event.method_name(), "stream.reasoning");

        let event = StreamEvent::ToolStart {
            run_id: "".to_string(),
            seq: 0,
            tool_name: "".to_string(),
            tool_id: "".to_string(),
            params: serde_json::json!({}),
        };
        assert_eq!(event.method_name(), "stream.tool_start");
    }

    #[test]
    fn test_reasoning_block_serialization() {
        let event = StreamEvent::reasoning_block(
            "run-123",
            1,
            ReasoningStepType::Analysis,
            "Analyzing options",
            "Comparing Redis vs in-memory cache",
            false,
        );

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("reasoning_block"));
        assert!(json.contains("analysis"));
        assert!(json.contains("Analyzing options"));
    }

    #[test]
    fn test_uncertainty_action_description() {
        assert!(UncertaintyAction::ProceedWithCaution
            .description()
            .contains("caution"));
        assert!(UncertaintyAction::AskForClarification
            .description()
            .contains("clarification"));
    }

    #[test]
    fn test_enhanced_run_summary() {
        let basic = RunSummary {
            total_tokens: 1000,
            tool_calls: 5,
            loops: 2,
            final_response: Some("Done".to_string()),
        };

        let mut enhanced = EnhancedRunSummary::from_basic(&basic, 5000);
        assert_eq!(enhanced.total_tokens, 1000);
        assert_eq!(enhanced.duration_ms, 5000);
        assert!(!enhanced.has_errors());

        enhanced.add_error(ToolErrorItem {
            tool_name: "test".to_string(),
            error: "failed".to_string(),
            tool_id: "t1".to_string(),
        });
        assert!(enhanced.has_errors());
    }

    #[test]
    fn test_config_changed_event_serde() {
        let event = ConfigChangedEvent {
            section: Some("ui.theme".to_string()),
            value: serde_json::json!({"color": "dark"}),
            timestamp: 1735689600,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ConfigChangedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.section, Some("ui.theme".to_string()));
    }
}

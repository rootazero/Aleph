//! Data types for streaming events
//!
//! Contains all enum variants, structs, and helper types used by the
//! event emitter subsystem.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sync_primitives::{AtomicU64, Ordering};

/// Error type for event emission
#[derive(Debug, thiserror::Error)]
pub enum EventEmitError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Event bus error: {0}")]
    EventBus(String),
}

/// Confidence level for reasoning blocks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
    Unknown,
}

/// Semantic type of a reasoning step
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStepType {
    Observation,
    Analysis,
    Planning,
    Decision,
    Reflection,
    Verification,
}

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
        /// The text delta for this chunk
        content: String,
        /// Accumulated full text within the current iteration
        #[serde(default)]
        full_text: String,
        chunk_index: u32,
        is_final: bool,
        /// When true, send to user immediately as standalone message (intermediate progress).
        /// When false, buffer per existing behavior.
        #[serde(default)]
        is_intermediate: bool,
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
    ///
    /// This is the enhanced version of the basic Reasoning event,
    /// providing semantic structure for better UI rendering.
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
    ///
    /// Emitted when the AI explicitly expresses uncertainty,
    /// allowing the UI to prompt for user guidance.
    UncertaintySignal {
        run_id: String,
        seq: u64,
        /// What the AI is uncertain about
        uncertainty: String,
        /// Suggested action for handling the uncertainty
        suggested_action: UncertaintyAction,
    },

    /// Session was updated (new messages added)
    ///
    /// Emitted after a run completes so that UI sidebars can refresh
    /// their session list without polling.
    SessionUpdated {
        session_key: String,
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

/// Per-RunId sequence counter manager
pub struct RunSequenceManager {
    sequences: DashMap<String, AtomicU64>,
}

impl RunSequenceManager {
    pub fn new() -> Self {
        Self {
            sequences: DashMap::new(),
        }
    }

    /// Get next sequence number for a run
    pub fn next_seq(&self, run_id: &str) -> u64 {
        self.sequences
            .entry(run_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst)
    }

    /// Cleanup sequences for completed run
    pub fn cleanup(&self, run_id: &str) {
        self.sequences.remove(run_id);
    }
}

impl Default for RunSequenceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Output mode for controlling response delivery behavior
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMode {
    /// Stream response chunks with throttling (character-by-character feel)
    Typewriter,
    /// Buffer all chunks and deliver complete response at once
    Instant,
}

impl OutputMode {
    /// Parse from config string value
    pub fn from_config(s: &str) -> Self {
        match s {
            "instant" => Self::Instant,
            _ => Self::Typewriter,
        }
    }
}

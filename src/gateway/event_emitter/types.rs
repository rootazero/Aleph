//! Data types for streaming events
//!
//! Contains all enum variants, structs, and helper types used by the
//! event emitter subsystem.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::health::ModelInfo;
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

    /// Structured execution trace emitted directly by the agent loop.
    /// Carried as the protocol `AgentTraceEvent` (serde tag `kind`) so the
    /// wire payload matches what the panel and the persisted replay store
    /// deserialize. Converted from `LoopTraceEvent` at the emit boundary.
    AgentTrace {
        run_id: String,
        seq: u64,
        event: aleph_protocol::AgentTraceEvent,
    },

    /// Response text chunk (streaming output)
    ResponseChunk {
        run_id: String,
        seq: u64,
        /// Incremental text for this chunk
        delta: String,
        /// Accumulated full text within the current iteration
        #[serde(default)]
        full_text: String,
        /// Backward-compatible alias for delta (DEPRECATED — use delta instead)
        content: String,
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

    /// Model has been resolved (possibly via fallback)
    ///
    /// Emitted after the provider registry resolves the model,
    /// so the UI can display fallback indicators.
    ModelResolved {
        run_id: String,
        model_info: ModelInfo,
    },

    /// Provider chain failed transiently; the run is about to retry.
    ///
    /// Emitted at each run-loop fallback retry so interfaces can show
    /// "provider unreachable, retrying" instead of a silent thinking
    /// indicator while the retry ladder burns minutes.
    RunRetrying {
        run_id: String,
        seq: u64,
        /// Provider that just failed.
        provider: String,
        /// 1-based attempt about to run (`2..=max_attempts`).
        attempt: u32,
        /// Total dispatch attempts before the run gives up.
        max_attempts: u32,
        /// Short human-readable failure reason (truncated at emit site).
        reason: String,
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
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::ProceedWithCaution => "Proceeding with caution despite uncertainty",
            Self::AskForClarification => "Asking user for clarification",
            Self::UseSaferApproach => "Using a safer, more conservative approach",
            Self::WaitForUser => "Waiting for user guidance",
        }
    }
}

impl StreamEvent {
    /// Create a new `ReasoningBlock` event
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

    /// Create a new `ReasoningBlock` event with confidence
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

    /// Create a new `UncertaintySignal` event
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

    /// Create a new structured agent trace event
    pub fn agent_trace(
        run_id: impl Into<String>,
        seq: u64,
        event: crate::harness::trace::LoopTraceEvent,
    ) -> Self {
        Self::AgentTrace {
            run_id: run_id.into(),
            seq,
            event: event.into(),
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

/// Summary of a completed agent run.
///
/// Mirrors `aleph_protocol::RunSummary` field-for-field — kept in parallel
/// because the gateway internal `StreamEvent` enum is distinct from the
/// wire-protocol enum (see `events.rs` comment). Every new field carries
/// `#[serde(default)]` so legacy producers round-trip cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunSummary {
    pub total_tokens: u64,
    pub tool_calls: u32,
    pub loops: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_reason: Option<String>,
    /// Granular cap label when `terminate_reason` is the umbrella
    /// `"budget_exhausted_partial_result"`. See the protocol-side
    /// `aleph_protocol::RunSummary::terminate_detail` for full rationale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_breakdown: Option<aleph_protocol::TokenBreakdownView>,
    /// Current context-window occupancy (tokens) after the latest turn. Gauge
    /// numerator on the panel. `#[serde(default)]` so legacy payloads → 0.
    #[serde(default)]
    pub context_tokens: u32,
    /// Authoritative context-window size (tokens) for the run's model. Gauge
    /// denominator on the panel. `#[serde(default)]` so legacy payloads → 0.
    #[serde(default)]
    pub context_window: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_summaries: Vec<ToolSummaryItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ToolErrorItem>,
}

/// Tool execution summary item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummaryItem {
    pub tool_id: String,
    pub tool_name: String,
    pub emoji: String,
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

/// Per-RunId sequence counter manager
pub struct RunSequenceManager {
    sequences: DashMap<String, AtomicU64>,
}

impl RunSequenceManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequences: DashMap::new(),
        }
    }

    /// Get next sequence number for a run
    #[must_use]
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
    #[must_use]
    pub fn from_config(s: &str) -> Self {
        match s {
            "instant" => Self::Instant,
            _ => Self::Typewriter,
        }
    }
}

//! Step boundary types for execution tracking

use serde::{Deserialize, Serialize};

/// Step start marker - marks the beginning of an agent loop step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepStartPart {
    /// Step number in the current session
    pub step_id: usize,
    /// When the step started
    pub timestamp: i64,
    /// Associated file snapshot ID (for revert capability)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
}

impl StepStartPart {
    /// Create a new step start marker
    pub fn new(step_id: usize) -> Self {
        Self {
            step_id,
            timestamp: chrono::Utc::now().timestamp(),
            snapshot_id: None,
        }
    }

    /// Create with associated snapshot
    pub fn with_snapshot(step_id: usize, snapshot_id: String) -> Self {
        Self {
            step_id,
            timestamp: chrono::Utc::now().timestamp(),
            snapshot_id: Some(snapshot_id),
        }
    }
}

/// Reason why a step finished
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepFinishReason {
    /// Step completed successfully
    Completed,
    /// Step failed with an error
    Failed,
    /// User aborted the step
    UserAborted,
    /// Tool execution error
    ToolError,
    /// Maximum steps limit reached
    MaxStepsReached,
}

impl Default for StepFinishReason {
    fn default() -> Self {
        Self::Completed
    }
}

/// Token usage for a step (re-exported from event types or defined locally)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepTokenUsage {
    /// Input tokens consumed
    pub input_tokens: u64,
    /// Output tokens generated
    pub output_tokens: u64,
}

impl StepTokenUsage {
    /// Create new token usage
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }

    /// Total tokens used
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Step finish marker - marks the end of an agent loop step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFinishPart {
    /// Step number that finished
    pub step_id: usize,
    /// Reason for finishing
    pub reason: StepFinishReason,
    /// Token usage for this step (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<StepTokenUsage>,
    /// Duration of the step in milliseconds
    pub duration_ms: u64,
}

impl StepFinishPart {
    /// Create a new step finish marker
    pub fn new(step_id: usize, reason: StepFinishReason, duration_ms: u64) -> Self {
        Self {
            step_id,
            reason,
            tokens: None,
            duration_ms,
        }
    }

    /// Create with token usage
    pub fn with_tokens(
        step_id: usize,
        reason: StepFinishReason,
        duration_ms: u64,
        tokens: StepTokenUsage,
    ) -> Self {
        Self {
            step_id,
            reason,
            tokens: Some(tokens),
            duration_ms,
        }
    }
}

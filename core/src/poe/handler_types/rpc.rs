//! POE RPC parameter and result types

use serde::{Deserialize, Serialize};

use crate::poe::{PoeOutcome, SuccessManifest};

/// Parameters for poe.run request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeRunParams {
    /// Success manifest defining success criteria
    pub manifest: SuccessManifest,
    /// Natural language instruction for the worker
    pub instruction: String,
    /// Whether to stream events during execution (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// POE configuration overrides
    #[serde(default)]
    pub config: Option<PoeConfigParams>,
}

/// Optional POE configuration overrides
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoeConfigParams {
    /// Stuck detection window (number of attempts)
    #[serde(default)]
    pub stuck_window: Option<usize>,
    /// Maximum tokens budget
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

pub fn default_stream() -> bool {
    true
}

/// Result of poe.run request (immediate response)
#[derive(Debug, Clone, Serialize)]
pub struct PoeRunResult {
    /// Unique task identifier (from manifest)
    pub task_id: String,
    /// Session key for event subscription
    pub session_key: String,
    /// Timestamp when task was accepted
    pub accepted_at: String,
}

/// Parameters for poe.status request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeStatusParams {
    /// Task ID to query
    pub task_id: String,
}

/// Result of poe.status request
#[derive(Debug, Clone, Serialize)]
pub struct PoeStatusResult {
    /// Task ID
    pub task_id: String,
    /// Current status
    pub status: String,
    /// Elapsed time in milliseconds
    pub elapsed_ms: u64,
    /// Current attempt number (if running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_attempt: Option<u8>,
    /// Last distance score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_distance_score: Option<f32>,
    /// Final outcome (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PoeOutcome>,
}

/// Parameters for poe.cancel request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeCancelParams {
    /// Task ID to cancel
    pub task_id: String,
}

/// Result of poe.cancel request
#[derive(Debug, Clone, Serialize)]
pub struct PoeCancelResult {
    /// Task ID
    pub task_id: String,
    /// Whether the task was successfully cancelled
    pub cancelled: bool,
    /// Reason if cancellation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

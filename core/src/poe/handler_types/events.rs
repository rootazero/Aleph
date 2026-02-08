//! POE event types

use serde::Serialize;

use crate::poe::PoeOutcome;

/// Event emitted when a POE task is accepted
#[derive(Debug, Clone, Serialize)]
pub struct PoeAcceptedEvent {
    pub task_id: String,
    pub session_key: String,
    pub accepted_at: String,
    pub objective: String,
}

/// Event emitted for each POE step (P->O->E iteration)
#[derive(Debug, Clone, Serialize)]
pub struct PoeStepEvent {
    pub task_id: String,
    pub attempt: u8,
    pub phase: String, // "principle", "operation", "evaluation"
    pub message: String,
}

/// Event emitted after validation
#[derive(Debug, Clone, Serialize)]
pub struct PoeValidationEvent {
    pub task_id: String,
    pub attempt: u8,
    pub passed: bool,
    pub distance_score: f32,
    pub reason: String,
}

/// Event emitted when POE task completes
#[derive(Debug, Clone, Serialize)]
pub struct PoeCompletedEvent {
    pub task_id: String,
    pub outcome: PoeOutcome,
    pub duration_ms: u64,
}

/// Event emitted on error
#[derive(Debug, Clone, Serialize)]
pub struct PoeErrorEvent {
    pub task_id: String,
    pub error: String,
}

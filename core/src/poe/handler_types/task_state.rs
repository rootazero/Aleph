//! POE task state types

use std::time::Instant;

use crate::poe::PoeOutcome;

/// Status of a POE task
#[derive(Debug, Clone)]
pub enum PoeTaskStatus {
    /// Task is queued/running
    Running {
        current_attempt: u8,
        last_distance_score: Option<f32>,
    },
    /// Task completed with outcome
    Completed(PoeOutcome),
    /// Task was cancelled
    Cancelled,
}

impl PoeTaskStatus {
    /// Get status string for serialization
    pub fn status_str(&self) -> &'static str {
        match self {
            PoeTaskStatus::Running { .. } => "running",
            PoeTaskStatus::Completed(outcome) => match outcome {
                PoeOutcome::Success { .. } => "success",
                PoeOutcome::StrategySwitch { .. } => "strategy_switch",
                PoeOutcome::BudgetExhausted { .. } => "budget_exhausted",
                PoeOutcome::DecompositionRequired { .. } => "decomposition_required",
            },
            PoeTaskStatus::Cancelled => "cancelled",
        }
    }
}

/// State of a POE task
#[derive(Debug, Clone)]
pub struct PoeTaskState {
    /// Task ID
    pub task_id: String,
    /// Session key for events
    pub session_key: String,
    /// When the task started
    pub started_at: Instant,
    /// Current status
    pub status: PoeTaskStatus,
    /// Whether streaming is enabled
    pub stream: bool,
}

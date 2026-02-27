//! Agent Loop execution result types

use crate::agent_loop::guards::GuardViolation;

/// Result of an Agent Loop execution
#[derive(Debug, Clone)]
pub enum LoopResult {
    /// Task completed successfully
    Completed {
        /// Summary of what was accomplished
        summary: String,
        /// Number of steps taken
        steps: usize,
        /// Total tokens consumed
        total_tokens: usize,
    },
    /// Task failed
    Failed {
        /// Reason for failure
        reason: String,
        /// Number of steps taken before failure
        steps: usize,
    },
    /// Guard triggered (resource limit hit)
    GuardTriggered(GuardViolation),
    /// User aborted the loop
    UserAborted,
    /// POE interceptor aborted the loop
    PoeAborted {
        /// Reason for POE-initiated abort
        reason: String,
    },
}

impl LoopResult {
    /// Check if result is successful
    pub fn is_success(&self) -> bool {
        matches!(self, LoopResult::Completed { .. })
    }

    /// Get step count
    pub fn steps(&self) -> usize {
        match self {
            LoopResult::Completed { steps, .. } => *steps,
            LoopResult::Failed { steps, .. } => *steps,
            LoopResult::GuardTriggered(_) => 0,
            LoopResult::UserAborted => 0,
            LoopResult::PoeAborted { .. } => 0,
        }
    }
}

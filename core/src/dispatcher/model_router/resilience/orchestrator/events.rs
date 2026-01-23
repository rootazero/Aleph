//! Orchestrator Events
//!
//! This module contains event types and callback handling for the orchestrator.

use serde::{Deserialize, Serialize};

// =============================================================================
// Orchestrator Event
// =============================================================================

/// Events emitted by the orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorEvent {
    /// Execution started
    ExecutionStarted {
        request_id: String,
        preferred_model: String,
    },

    /// Retry attempt starting
    RetryAttempt {
        request_id: String,
        attempt: u32,
        model_id: String,
        reason: String,
        backoff_ms: u64,
    },

    /// Failover to different model
    Failover {
        request_id: String,
        from_model: String,
        to_model: String,
        reason: String,
    },

    /// Execution completed successfully
    ExecutionSuccess {
        request_id: String,
        model_id: String,
        attempts: u32,
        duration_ms: u64,
    },

    /// Execution failed
    ExecutionFailed {
        request_id: String,
        error: String,
        attempts: u32,
        duration_ms: u64,
    },

    /// Circuit breaker skipped model
    CircuitBreakerSkip {
        request_id: String,
        model_id: String,
    },

    /// Budget warning
    BudgetWarning { request_id: String, message: String },
}

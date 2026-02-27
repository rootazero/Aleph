//! Crystallization projector — writes POE outcomes to LanceDB for experience replay.
//!
//! Listens for `OutcomeRecorded` events and projects them into the
//! `poe_experiences` LanceDB table with embedding vectors for similarity search.

use tracing::{debug, warn};

use crate::poe::events::{PoeEvent, PoeEventEnvelope};

/// Projector that crystallizes POE outcomes into the poe_experiences LanceDB table.
///
/// Only responds to `OutcomeRecorded` events. All other events are silently ignored.
///
/// The projector:
/// 1. Extracts outcome metrics from the event
/// 2. Generates an embedding from the task objective
/// 3. Writes a row to the poe_experiences table
pub struct CrystallizationProjector {
    // LanceDB table handle and embedder will be added when ExperienceStore is ready (Task 7)
}

impl CrystallizationProjector {
    /// Create a new CrystallizationProjector.
    pub fn new() -> Self {
        Self {}
    }

    /// Handle a POE event.
    ///
    /// Only processes `OutcomeRecorded` events. Returns `Ok(true)` if
    /// the event was processed, `Ok(false)` if skipped.
    pub async fn handle(&self, envelope: &PoeEventEnvelope) -> Result<bool, String> {
        match &envelope.event {
            PoeEvent::OutcomeRecorded {
                task_id,
                outcome,
                attempts,
                total_tokens: _,
                duration_ms,
                best_distance,
            } => {
                debug!(
                    task_id = %task_id,
                    outcome = ?outcome,
                    attempts = attempts,
                    duration_ms = duration_ms,
                    best_distance = best_distance,
                    "CrystallizationProjector: recording outcome"
                );

                // TODO: Generate embedding and write to poe_experiences table
                // This will be wired in Task 7 (ExperienceStore) and Task 8 (wiring)
                warn!("CrystallizationProjector: write not yet implemented (pending ExperienceStore)");

                Ok(true)
            }
            _ => Ok(false), // Ignore non-OutcomeRecorded events
        }
    }
}

impl Default for CrystallizationProjector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};

    #[tokio::test]
    async fn test_handles_outcome_recorded() {
        let projector = CrystallizationProjector::new();
        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "t1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 5000,
                duration_ms: 2000,
                best_distance: 0.1,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled, "Should handle OutcomeRecorded events");
    }

    #[tokio::test]
    async fn test_ignores_non_outcome_events() {
        let projector = CrystallizationProjector::new();
        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "t1".into(),
                objective: "test".into(),
                hard_constraints_count: 1,
                soft_metrics_count: 0,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(!handled, "Should ignore non-OutcomeRecorded events");
    }

    #[tokio::test]
    async fn test_ignores_pulse_events() {
        let projector = CrystallizationProjector::new();
        let envelope = PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OperationAttempted {
                task_id: "t1".into(),
                attempt: 1,
                tokens_used: 1000,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(!handled, "Should ignore Pulse events");
    }
}

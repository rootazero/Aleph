//! Trust projector — updates trust scores based on POE outcomes.
//!
//! Listens for `OutcomeRecorded` events and updates pattern-level
//! success metrics in the poe_trust_scores SQLite table.

use std::sync::Arc;
use tracing::{debug, warn};

use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};
use crate::resilience::database::StateDatabase;

/// Projector that maintains trust scores for task patterns.
///
/// Consumes `OutcomeRecorded` events and updates the poe_trust_scores
/// table with success/failure counts and derived trust score.
pub struct TrustProjector {
    db: Arc<StateDatabase>,
}

impl TrustProjector {
    /// Create a new TrustProjector.
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
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
                ..
            } => {
                let pattern_id = extract_pattern_id(task_id);
                let success = matches!(outcome, PoeOutcomeKind::Success);

                debug!(
                    task_id = %task_id,
                    pattern_id = %pattern_id,
                    success = success,
                    "TrustProjector: updating trust score"
                );

                match self.db.upsert_trust_score(&pattern_id, success).await {
                    Ok(new_score) => {
                        debug!(
                            pattern_id = %pattern_id,
                            new_score = new_score,
                            "TrustProjector: trust score updated"
                        );
                        Ok(true)
                    }
                    Err(e) => {
                        warn!(
                            pattern_id = %pattern_id,
                            error = %e,
                            "TrustProjector: failed to update trust score"
                        );
                        Err(format!("Failed to update trust score: {}", e))
                    }
                }
            }
            _ => Ok(false),
        }
    }
}

/// Extract a pattern ID from a task ID.
///
/// Simple heuristic: use the task_id as-is for now.
/// In the future, this could extract a normalized pattern
/// (e.g., "poe-create-rust-file" from "poe-task-12345").
fn extract_pattern_id(task_id: &str) -> String {
    task_id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};
    use crate::resilience::database::StateDatabase;

    fn make_db() -> Arc<StateDatabase> {
        Arc::new(StateDatabase::in_memory().unwrap())
    }

    #[tokio::test]
    async fn test_handles_outcome_recorded() {
        let db = make_db();
        let projector = TrustProjector::new(db.clone());

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 5000,
                duration_ms: 2000,
                best_distance: 0.1,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled);

        // Verify trust score was written
        let score = db.get_trust_score("task-1").await.unwrap().unwrap();
        assert_eq!(score.trust_score, 1.0);
        assert_eq!(score.total_executions, 1);
    }

    #[tokio::test]
    async fn test_ignores_non_outcome_events() {
        let db = make_db();
        let projector = TrustProjector::new(db);

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "task-1".into(),
                objective: "test".into(),
                hard_constraints_count: 1,
                soft_metrics_count: 0,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(!handled);
    }

    #[tokio::test]
    async fn test_trust_score_decreases_on_failure() {
        let db = make_db();
        let projector = TrustProjector::new(db.clone());

        // First: success
        let e1 = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 5000,
                duration_ms: 2000,
                best_distance: 0.1,
            },
            None,
        );
        projector.handle(&e1).await.unwrap();

        // Second: failure
        let e2 = PoeEventEnvelope::new(
            "task-1".into(),
            1,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::BudgetExhausted,
                attempts: 5,
                total_tokens: 50000,
                duration_ms: 30000,
                best_distance: 0.8,
            },
            None,
        );
        projector.handle(&e2).await.unwrap();

        let score = db.get_trust_score("task-1").await.unwrap().unwrap();
        assert_eq!(score.total_executions, 2);
        assert_eq!(score.successful_executions, 1);
        assert!((score.trust_score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_extract_pattern_id() {
        assert_eq!(extract_pattern_id("task-123"), "task-123");
    }
}

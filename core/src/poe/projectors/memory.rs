//! Memory projector — creates Memory facts from POE outcomes.
//!
//! Listens for `OutcomeRecorded` events and generates memory facts
//! for successful executions (core tier) and budget exhaustion (lessons learned).

use crate::sync_primitives::Arc;
use tracing::debug;

use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};

/// Callback trait for persisting memory facts from POE outcomes.
///
/// Decouples the projector from the actual Memory system implementation.
#[async_trait::async_trait]
pub trait MemoryFactWriter: Send + Sync {
    /// Write a POE outcome as a memory fact.
    ///
    /// # Arguments
    /// * `task_id` - The POE task ID
    /// * `content` - The fact content (human-readable summary)
    /// * `fact_type` - Either "poe_experience" or "lessons_learned"
    /// * `confidence` - Confidence score (0.0-1.0)
    async fn write_poe_fact(
        &self,
        task_id: &str,
        content: &str,
        fact_type: &str,
        confidence: f32,
    ) -> Result<(), String>;
}

/// No-op writer for when Memory integration is disabled.
pub struct NoOpMemoryFactWriter;

#[async_trait::async_trait]
impl MemoryFactWriter for NoOpMemoryFactWriter {
    async fn write_poe_fact(
        &self,
        _task_id: &str,
        _content: &str,
        _fact_type: &str,
        _confidence: f32,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Projector that creates Memory facts from POE outcomes.
///
/// Consumes `OutcomeRecorded` events and writes memory facts via the
/// [`MemoryFactWriter`] trait:
/// - **Success** → "poe_experience" fact with 0.9 confidence
/// - **BudgetExhausted** → "lessons_learned" fact with 0.7 confidence
/// - **StrategySwitch** → skipped (no memory created)
pub struct MemoryProjector {
    writer: Arc<dyn MemoryFactWriter>,
}

impl MemoryProjector {
    /// Create a new MemoryProjector with the given fact writer.
    pub fn new(writer: Arc<dyn MemoryFactWriter>) -> Self {
        Self { writer }
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
                duration_ms,
                best_distance,
                ..
            } => match outcome {
                PoeOutcomeKind::Success => {
                    let content = format!(
                        "POE task '{}' completed successfully in {} attempt(s) ({:.1}s). \
                         Final distance score: {:.3}.",
                        task_id,
                        attempts,
                        *duration_ms as f64 / 1000.0,
                        best_distance,
                    );
                    debug!(task_id = %task_id, "MemoryProjector: recording success fact");
                    self.writer
                        .write_poe_fact(task_id, &content, "poe_experience", 0.9)
                        .await?;
                    Ok(true)
                }
                PoeOutcomeKind::BudgetExhausted => {
                    let content = format!(
                        "POE task '{}' failed after {} attempt(s) ({:.1}s). \
                         Budget exhausted. Best distance: {:.3}. \
                         Consider a different approach.",
                        task_id,
                        attempts,
                        *duration_ms as f64 / 1000.0,
                        best_distance,
                    );
                    debug!(task_id = %task_id, "MemoryProjector: recording lessons learned");
                    self.writer
                        .write_poe_fact(task_id, &content, "lessons_learned", 0.7)
                        .await?;
                    Ok(true)
                }
                PoeOutcomeKind::StrategySwitch => {
                    debug!(task_id = %task_id, "MemoryProjector: skipping StrategySwitch");
                    Ok(false)
                }
                PoeOutcomeKind::DecompositionRequired => {
                    debug!(task_id = %task_id, "MemoryProjector: skipping DecompositionRequired");
                    Ok(false)
                }
            },
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicUsize, Ordering};

    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};

    /// Test writer that counts calls.
    struct CountingWriter {
        count: AtomicUsize,
    }

    impl CountingWriter {
        fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.count.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl MemoryFactWriter for CountingWriter {
        async fn write_poe_fact(
            &self,
            _task_id: &str,
            _content: &str,
            _fact_type: &str,
            _confidence: f32,
        ) -> Result<(), String> {
            self.count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_handles_success_outcome() {
        let writer = Arc::new(CountingWriter::new());
        let projector = MemoryProjector::new(writer.clone());

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 2,
                total_tokens: 5000,
                duration_ms: 3000,
                best_distance: 0.05,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled);
        assert_eq!(writer.call_count(), 1);
    }

    #[tokio::test]
    async fn test_handles_budget_exhausted() {
        let writer = Arc::new(CountingWriter::new());
        let projector = MemoryProjector::new(writer.clone());

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
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

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled);
        assert_eq!(writer.call_count(), 1);
    }

    #[tokio::test]
    async fn test_skips_strategy_switch() {
        let writer = Arc::new(CountingWriter::new());
        let projector = MemoryProjector::new(writer.clone());

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::StrategySwitch,
                attempts: 3,
                total_tokens: 15000,
                duration_ms: 10000,
                best_distance: 0.5,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(!handled);
        assert_eq!(writer.call_count(), 0);
    }

    #[tokio::test]
    async fn test_ignores_non_outcome_events() {
        let writer = Arc::new(CountingWriter::new());
        let projector = MemoryProjector::new(writer.clone());

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
        assert_eq!(writer.call_count(), 0);
    }

    #[tokio::test]
    async fn test_noop_writer() {
        let writer = Arc::new(NoOpMemoryFactWriter);
        let projector = MemoryProjector::new(writer);

        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.0,
            },
            None,
        );

        // Should not panic
        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled);
    }
}

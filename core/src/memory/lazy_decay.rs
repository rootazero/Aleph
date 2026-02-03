//! Lazy Decay Engine
//!
//! Calculates memory strength at read-time and asynchronously
//! invalidates decayed facts without blocking retrieval.

use crate::error::AetherError;
use crate::memory::context::MemoryFact;
use crate::memory::database::VectorDatabase;
use crate::memory::decay::{DecayConfig, MemoryStrength};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Result of lazy decay evaluation
#[derive(Debug)]
pub struct DecayEvaluation {
    /// Facts that passed decay check (still valid)
    pub valid_facts: Vec<MemoryFact>,
    /// Fact IDs that should be invalidated
    pub pending_invalidations: Vec<String>,
    /// Access updates to apply
    pub pending_access_updates: Vec<(String, i64)>, // (fact_id, timestamp)
}

struct InvalidationTask {
    fact_id: String,
    timestamp: i64,
}

/// Lazy decay processor
pub struct LazyDecayEngine {
    config: DecayConfig,
    db: Arc<VectorDatabase>,
    /// Channel for async invalidation tasks
    invalidation_tx: mpsc::Sender<InvalidationTask>,
}

impl LazyDecayEngine {
    /// Create a new lazy decay engine
    pub fn new(config: DecayConfig, db: Arc<VectorDatabase>) -> Self {
        let (tx, mut rx) = mpsc::channel::<InvalidationTask>(100);

        // Spawn background task for async invalidations
        let db_clone = db.clone();
        tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                if let Err(e) = db_clone
                    .soft_delete_fact(&task.fact_id, "decay", Some(task.timestamp))
                    .await
                {
                    tracing::warn!(
                        fact_id = %task.fact_id,
                        error = %e,
                        "Failed to invalidate decayed fact"
                    );
                }
            }
        });

        Self {
            config,
            db,
            invalidation_tx: tx,
        }
    }

    /// Evaluate decay for a batch of facts
    ///
    /// Returns valid facts and queues invalidations asynchronously.
    pub async fn evaluate(&self, facts: Vec<MemoryFact>) -> DecayEvaluation {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let mut valid_facts = Vec::new();
        let mut pending_invalidations = Vec::new();
        let mut pending_access_updates = Vec::new();

        for fact in facts {
            // Skip already invalid facts
            if !fact.is_valid {
                continue;
            }

            // Calculate current strength
            let strength = MemoryStrength {
                access_count: 0, // TODO: track in fact
                last_accessed: fact.updated_at,
                creation_time: fact.created_at,
            };

            let current_strength =
                strength.calculate_strength_for_type(&self.config, now, &fact.fact_type);

            if current_strength < self.config.min_strength {
                // Queue for async invalidation
                pending_invalidations.push(fact.id.clone());

                let _ = self
                    .invalidation_tx
                    .send(InvalidationTask {
                        fact_id: fact.id.clone(),
                        timestamp: now,
                    })
                    .await;
            } else {
                // Valid fact - queue access update
                pending_access_updates.push((fact.id.clone(), now));
                valid_facts.push(fact);
            }
        }

        DecayEvaluation {
            valid_facts,
            pending_invalidations,
            pending_access_updates,
        }
    }

    /// Batch update access timestamps (call after retrieval completes)
    pub async fn apply_access_updates(
        &self,
        updates: Vec<(String, i64)>,
    ) -> Result<(), AetherError> {
        for (fact_id, timestamp) in updates {
            self.db.update_fact_access(&fact_id, timestamp).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests require database setup - see integration_tests.rs
}

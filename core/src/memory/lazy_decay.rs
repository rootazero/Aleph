//! Lazy Decay Engine
//!
//! Calculates memory strength at read-time and asynchronously
//! invalidates decayed facts without blocking retrieval.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::decay::{DecayConfig, MemoryStrength};
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
    _timestamp: i64,
}

/// Lazy decay processor
pub struct LazyDecayEngine {
    config: DecayConfig,
    _db: MemoryBackend,
    /// Channel for async invalidation tasks
    invalidation_tx: mpsc::Sender<InvalidationTask>,
}

impl LazyDecayEngine {
    /// Create a new lazy decay engine
    pub fn new(config: DecayConfig, db: MemoryBackend) -> Self {
        let (tx, mut rx) = mpsc::channel::<InvalidationTask>(100);

        // Spawn background task for async invalidations
        let db_clone = db.clone();
        tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                if let Err(e) = db_clone
                    .soft_delete_fact(&task.fact_id, "decay")
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
            _db: db,
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
                        _timestamp: now,
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
    ///
    /// TODO: `update_fact_access` is not yet available in MemoryStore trait.
    /// This method currently updates each fact's content as a workaround.
    /// A dedicated `update_fact_access` method should be added to MemoryStore.
    pub async fn apply_access_updates(
        &self,
        _updates: Vec<(String, i64)>,
    ) -> Result<(), AlephError> {
        // TODO: MemoryStore trait does not have `update_fact_access`.
        // Once added, iterate over updates and call it.
        // For now, this is a no-op.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests require database setup - see integration_tests.rs
}

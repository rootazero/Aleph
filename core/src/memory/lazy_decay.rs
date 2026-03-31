//! Lazy Decay Engine
//!
//! Calculates memory strength at read-time and asynchronously
//! invalidates decayed facts without blocking retrieval.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::decay::{DecayConfig, MemoryStrength, TieredDecayConfig};
use crate::memory::store::{MemoryBackend, MemoryStore};
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
    /// Optional tiered config; when present, `evaluate()` uses tier-aware decay.
    tiered_config: Option<TieredDecayConfig>,
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
                if let Err(e) = db_clone.soft_delete_fact(&task.fact_id, "decay").await {
                    tracing::warn!(
                        fact_id = %task.fact_id,
                        error = %e,
                        "Failed to invalidate decayed fact"
                    );
                }
            }
            tracing::debug!("LazyDecayEngine invalidation channel closed, background task exiting");
        });

        Self {
            config,
            tiered_config: None,
            _db: db,
            invalidation_tx: tx,
        }
    }

    /// Create a new lazy decay engine with tiered config.
    ///
    /// When tiered config is set, `evaluate()` uses per-tier half-lives and
    /// access-reinforcement instead of the flat `DecayConfig`.
    pub fn with_tiered_config(mut self, tiered_config: TieredDecayConfig) -> Self {
        self.tiered_config = Some(tiered_config);
        self
    }

    /// Evaluate decay for a batch of facts
    ///
    /// Returns valid facts and queues invalidations asynchronously.
    pub async fn evaluate(&self, facts: Vec<MemoryFact>) -> DecayEvaluation {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
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
                access_count: fact.access_count,
                last_accessed: fact.last_accessed_at.unwrap_or(fact.updated_at),
                creation_time: fact.created_at,
            };

            let (current_strength, min_strength) = if let Some(ref tc) = self.tiered_config {
                let s = strength.calculate_strength_tiered(tc, &fact.tier, &fact.fact_type, now);
                let ms = tc.params_for_tier(&fact.tier).min_strength;
                (s, ms)
            } else {
                let s = strength.calculate_strength_for_type(&self.config, now, &fact.fact_type);
                (s, self.config.min_strength)
            };

            if current_strength < min_strength {
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

    /// Batch update access timestamps (call after retrieval completes).
    ///
    /// NOTE: This is currently a no-op because `MemoryStore` does not have a
    /// dedicated `update_fact_access` method yet. As a result, `access_count`
    /// and `last_accessed_at` are never updated, and lazy decay treats all
    /// facts as never-accessed. Adding `update_fact_access` to `MemoryStore`
    /// will fix this.
    pub async fn apply_access_updates(
        &self,
        updates: Vec<(String, i64)>,
    ) -> Result<(), AlephError> {
        if !updates.is_empty() {
            tracing::debug!(
                count = updates.len(),
                "apply_access_updates is a no-op — access counts not yet persisted"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests require database setup - see integration_tests.rs
}

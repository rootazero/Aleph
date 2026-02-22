//! Evolution resolver for handling contradictions

use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::Result;

use super::chain::{EvolutionChain, FactEvolution};

/// Resolves contradictions by creating evolution chains
pub struct EvolutionResolver {
    database: MemoryBackend,
}

/// Resolution strategy for handling contradictions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum ResolutionStrategy {
    /// Keep the newer fact, invalidate the older one
    PreferNewer,

    /// Keep the fact with higher confidence
    PreferHigherConfidence,

    /// Keep both facts but mark them as part of an evolution chain
    #[default]
    CreateEvolution,
}


impl EvolutionResolver {
    /// Create a new evolution resolver
    pub fn new(database: MemoryBackend) -> Self {
        Self { database }
    }

    /// Resolve a contradiction between two facts
    ///
    /// Returns the evolution chain created from the resolution
    pub async fn resolve(
        &self,
        old_fact: MemoryFact,
        new_fact: MemoryFact,
        reason: String,
        strategy: ResolutionStrategy,
    ) -> Result<FactEvolution> {
        match strategy {
            ResolutionStrategy::PreferNewer => {
                // Invalidate the old fact
                self.database
                    .invalidate_fact(&old_fact.id, &format!("Superseded: {}", reason))
                    .await?;

                // Create evolution chain
                Ok(EvolutionChain::create_evolution(old_fact, new_fact, reason))
            }

            ResolutionStrategy::PreferHigherConfidence => {
                // Compare confidence scores
                if new_fact.confidence > old_fact.confidence {
                    self.database
                        .invalidate_fact(&old_fact.id, &format!("Lower confidence: {}", reason))
                        .await?;
                    Ok(EvolutionChain::create_evolution(old_fact, new_fact, reason))
                } else {
                    self.database
                        .invalidate_fact(&new_fact.id, &format!("Lower confidence: {}", reason))
                        .await?;
                    Ok(EvolutionChain::create_evolution(new_fact, old_fact, reason))
                }
            }

            ResolutionStrategy::CreateEvolution => {
                // Keep both facts but create evolution chain
                // The old fact is marked as superseded but not invalidated
                Ok(EvolutionChain::create_evolution(old_fact, new_fact, reason))
            }
        }
    }

    /// Resolve multiple contradictions for a new fact
    pub async fn resolve_multiple(
        &self,
        new_fact: MemoryFact,
        contradictions: Vec<(MemoryFact, String)>,
        strategy: ResolutionStrategy,
    ) -> Result<Vec<FactEvolution>> {
        let mut evolutions = Vec::new();

        for (old_fact, reason) in contradictions {
            let evolution = self
                .resolve(old_fact, new_fact.clone(), reason, strategy)
                .await?;
            evolutions.push(evolution);
        }

        Ok(evolutions)
    }
}

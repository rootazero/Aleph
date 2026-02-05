//! Evolution chain structures and management

use crate::memory::MemoryFact;
use serde::{Deserialize, Serialize};

/// A chain of fact evolution showing how knowledge changed over time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactEvolution {
    /// Unique identifier for this evolution chain
    pub chain_id: String,

    /// Ordered list of facts in the evolution (oldest to newest)
    pub facts: Vec<EvolutionNode>,

    /// When this evolution chain was created
    pub created_at: i64,
}

/// A node in the evolution chain representing one version of a fact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionNode {
    /// ID of the fact
    pub fact_id: String,

    /// The actual fact content
    pub fact: MemoryFact,

    /// ID of the fact that superseded this one (if any)
    pub superseded_by: Option<String>,

    /// Reason for the evolution/contradiction
    pub reason: String,

    /// When this node was added to the chain
    pub timestamp: i64,
}

/// Manages evolution chains
pub struct EvolutionChain;

impl EvolutionChain {
    /// Create a new evolution chain from an old fact and a new fact
    pub fn create_evolution(
        old_fact: MemoryFact,
        new_fact: MemoryFact,
        reason: String,
    ) -> FactEvolution {
        let chain_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        let old_node = EvolutionNode {
            fact_id: old_fact.id.clone(),
            fact: old_fact,
            superseded_by: Some(new_fact.id.clone()),
            reason: reason.clone(),
            timestamp: now,
        };

        let new_node = EvolutionNode {
            fact_id: new_fact.id.clone(),
            fact: new_fact,
            superseded_by: None,
            reason,
            timestamp: now,
        };

        FactEvolution {
            chain_id,
            facts: vec![old_node, new_node],
            created_at: now,
        }
    }

    /// Add a new fact to an existing evolution chain
    pub fn extend_evolution(
        mut evolution: FactEvolution,
        new_fact: MemoryFact,
        reason: String,
    ) -> FactEvolution {
        let now = chrono::Utc::now().timestamp();

        // Mark the last fact as superseded
        if let Some(last_node) = evolution.facts.last_mut() {
            last_node.superseded_by = Some(new_fact.id.clone());
        }

        // Add the new fact
        let new_node = EvolutionNode {
            fact_id: new_fact.id.clone(),
            fact: new_fact,
            superseded_by: None,
            reason,
            timestamp: now,
        };

        evolution.facts.push(new_node);
        evolution
    }

    /// Get the current (most recent) fact in the evolution chain
    pub fn get_current_fact(evolution: &FactEvolution) -> Option<&MemoryFact> {
        evolution.facts.last().map(|node| &node.fact)
    }

    /// Get the history of a fact (all previous versions)
    pub fn get_history(evolution: &FactEvolution) -> Vec<&MemoryFact> {
        evolution.facts.iter().map(|node| &node.fact).collect()
    }
}

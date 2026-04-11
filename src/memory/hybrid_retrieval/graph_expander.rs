//! Graph-augmented retrieval expansion.
//!
//! Given a set of candidate facts from vector + FTS search, expands them
//! by traversing the knowledge graph to discover structurally related facts
//! that may not be semantically similar but are knowledge-linked.

use std::collections::HashSet;

use crate::error::AlephError;
use crate::memory::store::types::ScoredFact;
use crate::memory::store::{GraphStore, MemoryBackend, MemoryStore};

/// Configuration for graph expansion.
#[derive(Debug, Clone)]
pub struct GraphExpansionConfig {
    pub enabled: bool,
    pub max_hops: usize,
    pub max_expanded_per_seed: usize,
    pub max_total_expanded: usize,
    pub min_weight: f32,
    pub hop_decay: f32,
}

impl Default for GraphExpansionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_hops: 1,
            max_expanded_per_seed: 3,
            max_total_expanded: 10,
            min_weight: 0.3,
            hop_decay: 0.7,
        }
    }
}

/// A fact discovered through graph expansion.
#[derive(Debug, Clone)]
pub struct ExpandedFact {
    pub scored_fact: ScoredFact,
    pub seed_fact_id: String,
    pub expansion_path: String,
}

/// Expands retrieval results using knowledge graph traversal.
pub struct GraphExpander {
    database: MemoryBackend,
    config: GraphExpansionConfig,
}

impl GraphExpander {
    pub fn new(database: MemoryBackend, config: GraphExpansionConfig) -> Self {
        Self { database, config }
    }

    pub async fn expand(
        &self,
        seeds: &[ScoredFact],
        workspace: &str,
    ) -> Result<Vec<ExpandedFact>, AlephError> {
        if !self.config.enabled || seeds.is_empty() {
            return Ok(Vec::new());
        }

        let seed_ids: HashSet<String> = seeds.iter().map(|s| s.fact.id.clone()).collect();
        let mut all_expanded: Vec<ExpandedFact> = Vec::new();

        for seed in seeds {
            if all_expanded.len() >= self.config.max_total_expanded {
                break;
            }

            let mut per_seed_count = 0;

            // 1. Get graph nodes for this seed fact
            let nodes = self
                .database
                .get_nodes_for_fact(&seed.fact.id, workspace)
                .await?;

            for (node, link_weight) in &nodes {
                if per_seed_count >= self.config.max_expanded_per_seed {
                    break;
                }
                if *link_weight < self.config.min_weight {
                    continue;
                }

                // 2. Get edges from this node
                let edges = self
                    .database
                    .get_edges_for_node(&node.id, None, workspace)
                    .await?;

                for edge in &edges {
                    if per_seed_count >= self.config.max_expanded_per_seed {
                        break;
                    }

                    let neighbor_id = if edge.from_id == node.id {
                        &edge.to_id
                    } else {
                        &edge.from_id
                    };

                    // 3. Get facts for neighbor node
                    let neighbor_facts = self
                        .database
                        .get_facts_for_node(neighbor_id, workspace)
                        .await?;

                    for (fact_id, fact_link_weight) in &neighbor_facts {
                        if per_seed_count >= self.config.max_expanded_per_seed {
                            break;
                        }
                        if all_expanded.len() >= self.config.max_total_expanded {
                            break;
                        }
                        if seed_ids.contains(fact_id) {
                            continue;
                        }
                        if all_expanded.iter().any(|e| e.scored_fact.fact.id == *fact_id) {
                            continue;
                        }
                        if *fact_link_weight < self.config.min_weight {
                            continue;
                        }

                        // 4. Load and score
                        if let Ok(Some(fact)) = self.database.get_fact(fact_id).await {
                            if !fact.is_valid {
                                continue;
                            }

                            let expanded_score = seed.score
                                * edge.weight
                                * link_weight
                                * fact_link_weight
                                * self.config.hop_decay;

                            all_expanded.push(ExpandedFact {
                                scored_fact: ScoredFact {
                                    fact,
                                    score: expanded_score,
                                },
                                seed_fact_id: seed.fact.id.clone(),
                                expansion_path: format!(
                                    "via entity '{}' → {} → neighbor",
                                    node.name, edge.relation,
                                ),
                            });
                            per_seed_count += 1;
                        }
                    }
                }
            }
        }

        all_expanded.sort_by(|a, b| {
            b.scored_fact
                .score
                .partial_cmp(&a.scored_fact.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_expanded.truncate(self.config.max_total_expanded);

        Ok(all_expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_conservative_values() {
        let config = GraphExpansionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_hops, 1);
        assert_eq!(config.max_expanded_per_seed, 3);
        assert_eq!(config.max_total_expanded, 10);
        assert!((config.min_weight - 0.3).abs() < f32::EPSILON);
        assert!((config.hop_decay - 0.7).abs() < f32::EPSILON);
    }
}

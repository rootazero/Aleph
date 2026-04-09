//! RippleTask implementation for knowledge graph exploration

use std::collections::HashSet;

use crate::memory::context::MemoryFact;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::Result;

use super::config::{RippleConfig, RippleResult};

/// RippleTask explores related facts using vector similarity
pub struct RippleTask {
    database: MemoryBackend,
    config: RippleConfig,
}

impl RippleTask {
    /// Create a new RippleTask
    pub fn new(database: MemoryBackend, config: RippleConfig) -> Self {
        Self { database, config }
    }

    /// Explore related facts starting from seed facts
    ///
    /// Performs breadth-first exploration using vector similarity to find related facts.
    /// Each hop searches for facts similar to the current level's facts.
    pub async fn explore(&self, seed_facts: Vec<MemoryFact>) -> Result<RippleResult> {
        let mut visited = HashSet::new();
        let mut expanded = Vec::new();
        let mut current_level = seed_facts.clone();

        // Mark seed facts as visited
        for fact in &seed_facts {
            visited.insert(fact.id.clone());
        }

        // Perform BFS traversal using vector similarity
        for _hop in 0..self.config.max_hops {
            let mut next_level = Vec::new();

            for fact in &current_level {
                // Skip facts without embeddings
                let Some(embedding) = &fact.embedding else {
                    continue;
                };

                // Search for similar facts using vector_search
                let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner)); // TODO: Pass from context
                let dim_hint = embedding.len() as u32;
                let scored_facts = self
                    .database
                    .vector_search(embedding, dim_hint, &filter, self.config.max_facts_per_hop)
                    .await?;

                // Convert ScoredFact to MemoryFact, attaching similarity_score
                let similar_facts: Vec<MemoryFact> = scored_facts
                    .into_iter()
                    .map(|sf| {
                        let mut f = sf.fact;
                        f.similarity_score = Some(sf.score);
                        f
                    })
                    .collect();

                for similar_fact in similar_facts {
                    // Skip if already visited
                    if visited.contains(&similar_fact.id) {
                        continue;
                    }

                    // Check similarity threshold
                    if self.is_similar(fact, &similar_fact) {
                        visited.insert(similar_fact.id.clone());
                        expanded.push(similar_fact.clone());
                        next_level.push(similar_fact);
                    }
                }
            }

            // Move to next level
            current_level = next_level;

            // Stop if no more facts to explore
            if current_level.is_empty() {
                break;
            }
        }

        Ok(RippleResult {
            seed_facts,
            expanded_facts: expanded,
            total_hops: self.config.max_hops,
            tunnel_facts: Vec::new(),
        })
    }

    /// Explore cross-domain facts via tunnel edges in the knowledge graph.
    pub async fn explore_tunnels(
        &self,
        seed_facts: &[MemoryFact],
        graph_store: &crate::memory::graph::GraphStore,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<Vec<MemoryFact>> {
        if !self.config.enable_tunnels {
            return Ok(Vec::new());
        }

        let mut tunnel_facts = Vec::new();
        let max = self.config.max_facts_per_hop * self.config.max_tunnel_hops;

        'outer: for fact in seed_facts {
            let node_id = format!("fact:{}", fact.id);

            // Query graph for tunnel edges from this node
            let edges = graph_store.get_edges_by_relation(&node_id, "tunnel").await?;

            for edge in edges {
                let target_node = if edge.from_id == node_id {
                    &edge.to_id
                } else {
                    &edge.from_id
                };

                // Extract fact ID from node ID ("fact:uuid" -> "uuid")
                let target_fact_id = target_node.strip_prefix("fact:").unwrap_or(target_node);

                if visited.contains(target_fact_id) {
                    continue;
                }

                if let Ok(Some(target_fact)) = self.database.get_fact(target_fact_id).await {
                    if target_fact.is_valid && target_fact.is_currently_valid() {
                        visited.insert(target_fact_id.to_string());
                        tunnel_facts.push(target_fact);

                        if tunnel_facts.len() >= max {
                            break 'outer;
                        }
                    }
                }
            }
        }

        Ok(tunnel_facts)
    }

    /// Check if a candidate fact meets the similarity threshold.
    ///
    /// Uses the `similarity_score` populated by vector_search rather than
    /// re-computing cosine similarity, since SQLite-backed facts do not
    /// carry embeddings in the returned rows.
    fn is_similar(&self, _seed: &MemoryFact, candidate: &MemoryFact) -> bool {
        if let Some(score) = candidate.similarity_score {
            return score >= self.config.similarity_threshold;
        }

        // Fallback: compute from embeddings if both are present
        let (Some(emb1), Some(emb2)) = (&_seed.embedding, &candidate.embedding) else {
            return false;
        };

        let similarity = cosine_similarity(emb1, emb2);
        similarity >= self.config.similarity_threshold
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if !norm_a.is_finite() || !norm_b.is_finite() || norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    (dot_product / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        // Identical vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        // Orthogonal vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);

        // Opposite vectors
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 0.001);

        // Similar vectors
        let a = vec![1.0, 1.0, 0.0];
        let b = vec![1.0, 0.9, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim > 0.9 && sim < 1.0);
    }
}

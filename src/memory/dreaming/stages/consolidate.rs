//! ConsolidateStage: promotes STM facts to LTM via 8-dimensional scoring and prunes weak facts.
//!
//! Iterates all valid ShortTerm facts aged >= 24h and applies:
//! - **Pruning**: non-Core facts with strength < pruning threshold are invalidated (unchanged).
//! - **Promotion**: 8-dimensional PromotionScorer evaluates frequency, relevance, diversity,
//!   recency, consolidation, conceptual richness, graph centrality, and cluster cohesion
//!   to decide whether a ShortTerm fact should be promoted to LongTerm.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage, MemoryCluster};
use crate::error::AlephError;
use crate::memory::consolidation::promotion_scorer::{
    compute_cluster_cohesion, compute_conceptual, compute_consolidation, compute_diversity,
    compute_frequency, compute_graph_centrality, compute_recency, compute_relevance,
    PromotionScorer,
};
use crate::memory::context::MemoryTier;
use crate::memory::dreaming::{now_timestamp, should_prune, ConsolidationPipelineConfig};
use crate::memory::store::MemoryStore;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract potential entity names from fact content.
/// Heuristic: words starting with an uppercase letter that are at least 2 chars.
fn extract_entity_names(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| {
            w.len() >= 2
                && w.chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
        })
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Count conceptual tags: 1 for the fact_type + number of VFS path segments.
fn count_conceptual_tags(fact_type_name: &str, path: &str) -> u32 {
    let type_tag: u32 = if fact_type_name != "Other" { 1 } else { 0 };
    let path_segments = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != "aleph:")
        .count() as u32;
    type_tag + path_segments
}

/// Return the maximum edge count for any entity mentioned in the content.
fn max_entity_edges(content: &str, edge_counts: &HashMap<String, u32>) -> u32 {
    extract_entity_names(content)
        .iter()
        .filter_map(|name| edge_counts.get(name))
        .copied()
        .max()
        .unwrap_or(0)
}

/// Build a lookup from fact_id to (distance_to_centroid, cluster_radius).
///
/// For each cluster, compute the radius as the max distance from centroid to
/// any member. Then for each member that has an embedding, compute its distance
/// to the centroid and record the pair.
fn build_cluster_lookup(clusters: &[MemoryCluster]) -> HashMap<String, (f32, f32)> {
    let mut lookup = HashMap::new();

    for cluster in clusters {
        let centroid = match &cluster.centroid {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };

        // Collect (fact_id, distance) for members that have embeddings
        // MemoryCluster.members are MemoryEntry which may not have fact_id,
        // but we use the entry's id as a proxy key. The caller matches
        // fact.id against cluster member ids via source_memory_ids.
        let mut member_distances: Vec<(String, f32)> = Vec::new();
        for member in &cluster.members {
            if let Some(ref emb) = member.embedding {
                let dist = cosine_distance(emb, centroid);
                member_distances.push((member.id.clone(), dist));
            }
        }

        let radius = member_distances
            .iter()
            .map(|(_, d)| *d)
            .fold(0.0_f32, f32::max)
            .max(f32::EPSILON); // avoid zero radius

        for (member_id, dist) in member_distances {
            lookup.insert(member_id, (dist, radius));
        }
    }

    lookup
}

/// Cosine distance: 1.0 - cosine_similarity.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 1.0;
    }
    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        return 1.0;
    }
    1.0 - (dot / denom)
}

// ---------------------------------------------------------------------------
// ConsolidateStage
// ---------------------------------------------------------------------------

/// Consolidates short-term memory facts into long-term memory using
/// 8-dimensional promotion scoring.
pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = ConsolidationPipelineConfig::default();
        let scorer = PromotionScorer::default();
        let now = now_timestamp();
        let min_age_secs: i64 = scorer.thresholds.min_age_hours as i64 * 3600;

        // 1. Get all valid facts
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let facts: Vec<_> = all_facts
            .into_iter()
            .take(config.max_facts_per_run)
            .collect();

        // 2. Filter ShortTerm facts aged >= min_age_hours for promotion candidates
        let stm_candidates: Vec<_> = facts
            .iter()
            .filter(|f| {
                f.tier == MemoryTier::ShortTerm && (now - f.created_at) >= min_age_secs
            })
            .collect();

        // 3. Batch query recall signal aggregates for candidate fact IDs
        let candidate_ids: Vec<String> = stm_candidates.iter().map(|f| f.id.clone()).collect();
        let recall_aggregates = ctx.database.aggregate_for_facts(&candidate_ids)?;
        let recall_map: HashMap<String, _> = recall_aggregates
            .into_iter()
            .map(|agg| (agg.fact_id.clone(), agg))
            .collect();

        // 4. Batch query graph edge counts per workspace (agent)
        //    Group entity names by agent to avoid cross-workspace bias.
        let mut entities_by_agent: HashMap<String, Vec<String>> = HashMap::new();
        for f in &stm_candidates {
            let names = extract_entity_names(&f.content);
            if !names.is_empty() {
                entities_by_agent
                    .entry(f.agent.clone())
                    .or_default()
                    .extend(names);
            }
        }
        // Dedup entity names per agent
        for names in entities_by_agent.values_mut() {
            names.sort();
            names.dedup();
        }
        let mut edge_counts_by_agent: HashMap<String, HashMap<String, u32>> = HashMap::new();
        for (agent, names) in &entities_by_agent {
            let counts = ctx.database.edge_count_for_entities(names, agent)?;
            edge_counts_by_agent.insert(agent.clone(), counts);
        }

        // 5. Build cluster lookup from ctx.clusters
        let cluster_lookup = build_cluster_lookup(&ctx.clusters);

        // 6. Process each fact
        let mut consolidated_count: usize = 0;
        let mut pruned_count: usize = 0;

        for mut fact in facts {
            // 6a. Pruning check (unchanged)
            if should_prune(&fact, config.pruning_threshold) {
                ctx.database
                    .invalidate_fact(&fact.id, "strength below pruning threshold")
                    .await?;
                pruned_count += 1;
                continue;
            }

            // 6b. Only promote ShortTerm facts that are old enough
            if fact.tier != MemoryTier::ShortTerm || (now - fact.created_at) < min_age_secs {
                continue;
            }

            // Compute 8 dimensions
            let agg = recall_map.get(&fact.id);

            let signal_count = agg.map(|a| a.signal_count as u32).unwrap_or(0);
            let total_score = agg.map(|a| a.total_score as f32).unwrap_or(0.0);
            let unique_queries = agg.map(|a| a.unique_queries as u32).unwrap_or(0);
            let unique_channels = agg.map(|a| a.unique_channels as u32).unwrap_or(0);
            let recall_days = agg.map(|a| a.recall_days as u32).unwrap_or(0);
            let first_recall = agg.map(|a| a.first_recall).unwrap_or(now);
            let last_recall = agg.map(|a| a.last_recall).unwrap_or(now);

            let days_since_access =
                (now - fact.last_accessed_at.unwrap_or(fact.created_at)) as f32 / 86400.0;
            let span_days = (last_recall - first_recall).max(0) as f32 / 86400.0;
            let fact_type_name = format!("{:?}", fact.fact_type);
            let tag_count = count_conceptual_tags(&fact_type_name, &fact.path);
            let fact_edge_counts = edge_counts_by_agent
                .get(&fact.agent)
                .cloned()
                .unwrap_or_default();
            let edge_count = max_entity_edges(&fact.content, &fact_edge_counts);

            // Cluster cohesion: check if any source_memory_id is in the cluster lookup
            let (dist_to_centroid, cluster_radius) = fact
                .source_memory_ids
                .iter()
                .find_map(|mid| cluster_lookup.get(mid))
                .copied()
                .unwrap_or((0.0, 0.0));

            let dims = [
                compute_frequency(signal_count),
                compute_relevance(total_score, signal_count),
                compute_diversity(unique_queries, unique_channels),
                compute_recency(days_since_access),
                compute_consolidation(recall_days, span_days),
                compute_conceptual(tag_count),
                compute_graph_centrality(edge_count),
                compute_cluster_cohesion(dist_to_centroid, cluster_radius),
            ];

            let score = scorer.score(&dims);
            let age_hours = ((now - fact.created_at).max(0) as u64) / 3600;

            if scorer.should_promote(score, signal_count, unique_queries, age_hours) {
                fact.tier = MemoryTier::LongTerm;
                ctx.database.update_fact(&fact).await?;
                consolidated_count += 1;
            }
        }

        info!(
            consolidated = consolidated_count,
            pruned = pruned_count,
            "ConsolidateStage: STM→LTM consolidation complete"
        );

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidate_stage_name() {
        assert_eq!(ConsolidateStage.name(), "consolidate");
    }

    #[test]
    fn extract_entity_names_finds_capitalized_words() {
        let names = extract_entity_names("Alice met Bob at the Rust conference");
        assert!(names.contains(&"Alice".to_string()));
        assert!(names.contains(&"Bob".to_string()));
        assert!(names.contains(&"Rust".to_string()));
        // lowercase words should not appear
        assert!(!names.iter().any(|n| n == "met" || n == "at" || n == "the"));
    }

    #[test]
    fn extract_entity_names_handles_punctuation() {
        let names = extract_entity_names("Hello, World!");
        assert!(names.contains(&"Hello".to_string()));
        assert!(names.contains(&"World".to_string()));
    }

    #[test]
    fn count_conceptual_tags_with_path() {
        // fact_type "Learning" = 1, path "aleph://user/preferences/coding" = 3 segments
        let count = count_conceptual_tags("Learning", "aleph://user/preferences/coding");
        assert_eq!(count, 4); // 1 + 3
    }

    #[test]
    fn count_conceptual_tags_other_type() {
        let count = count_conceptual_tags("Other", "");
        assert_eq!(count, 0);
    }

    #[test]
    fn max_entity_edges_returns_max() {
        let mut edges = HashMap::new();
        edges.insert("Alice".to_string(), 5);
        edges.insert("Bob".to_string(), 3);
        let max = max_entity_edges("Alice and Bob went to the park", &edges);
        assert_eq!(max, 5);
    }

    #[test]
    fn max_entity_edges_no_match() {
        let edges = HashMap::new();
        let max = max_entity_edges("hello world", &edges);
        assert_eq!(max, 0);
    }

    #[test]
    fn cosine_distance_identical_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_distance(&a, &b)).abs() < 0.001);
    }

    #[test]
    fn cosine_distance_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_distance(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn build_cluster_lookup_empty() {
        let lookup = build_cluster_lookup(&[]);
        assert!(lookup.is_empty());
    }
}

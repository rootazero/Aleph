//! TunnelDiscoveryStage: discovers cross-domain concept bridges.
//!
//! When the same topic (e.g. "preferences") appears in multiple VFS domains
//! (e.g. `aleph://user/preferences/…` and `aleph://knowledge/preferences/…`),
//! this stage creates "tunnel" edges in the knowledge graph to bridge them.
//!
//! The algorithm:
//! 1. Fetch tunnel candidate facts from the database.
//! 2. Group candidates by topic (derived from VFS path).
//! 3. For each topic present in 2+ domains:
//!    a. Pick the representative fact per domain (highest strength).
//!    b. Compute pairwise embedding cosine similarity.
//!    c. If similarity >= 0.6, create a "tunnel" edge in the graph.
//! 4. Clear tunnel_pending flags for processed topics.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::{debug, info};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::parse_domain_topic;
use crate::memory::context::MemoryFact;
use crate::memory::store::MemoryStore;

/// Maximum number of tunnel candidates to process per dream run.
const TUNNEL_BATCH_SIZE: usize = 200;

/// Minimum cosine similarity to create a tunnel edge.
const TUNNEL_SIMILARITY_THRESHOLD: f32 = 0.6;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Group facts by their topic (second VFS path segment).
///
/// Returns a map from topic name to the list of facts belonging to that topic.
/// Facts with empty topics are silently skipped.
pub fn group_by_topic(facts: &[MemoryFact]) -> HashMap<String, Vec<&MemoryFact>> {
    let mut groups: HashMap<String, Vec<&MemoryFact>> = HashMap::new();
    for fact in facts {
        let parts: (&str, &str) = parse_domain_topic(&fact.path);
        if parts.1.is_empty() {
            continue;
        }
        groups.entry(parts.1.to_string()).or_default().push(fact);
    }
    groups
}

/// Group a slice of fact references by their domain (first VFS path segment).
///
/// Returns a map from domain name to the list of facts in that domain.
/// Facts with empty domains are silently skipped.
pub fn group_by_domain<'a>(facts: &[&'a MemoryFact]) -> HashMap<String, Vec<&'a MemoryFact>> {
    let mut groups: HashMap<String, Vec<&'a MemoryFact>> = HashMap::new();
    for fact in facts {
        let parts: (&str, &str) = parse_domain_topic(&fact.path);
        if parts.0.is_empty() {
            continue;
        }
        groups.entry(parts.0.to_string()).or_default().push(fact);
    }
    groups
}

/// Compute cosine similarity between two optional embedding vectors.
///
/// Returns 0.0 if either embedding is `None` or empty, or if both have zero magnitude.
pub fn embedding_similarity(a: Option<&[f32]>, b: Option<&[f32]>) -> f32 {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => (a, b),
        _ => return 0.0,
    };

    if a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut mag_a = 0.0_f64;
    let mut mag_b = 0.0_f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        mag_a += x * x;
        mag_b += y * y;
    }

    let denom = mag_a.sqrt() * mag_b.sqrt();
    if denom < f64::EPSILON {
        return 0.0;
    }

    (dot / denom) as f32
}

/// Pick the representative fact per domain (highest strength).
/// Returns `None` if the slice is empty.
fn pick_representative<'a>(facts: &[&'a MemoryFact]) -> Option<&'a MemoryFact> {
    facts
        .iter()
        .copied()
        .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal))
}

// ---------------------------------------------------------------------------
// TunnelDiscoveryStage
// ---------------------------------------------------------------------------

/// Discovers cross-domain concept tunnels and creates graph edges.
pub struct TunnelDiscoveryStage;

#[async_trait]
impl DreamStage for TunnelDiscoveryStage {
    fn name(&self) -> &'static str {
        "tunnel_discovery"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        match ctx.database.has_tunnel_pending().await {
            Ok(has) => has,
            Err(e) => {
                debug!(error = %e, "TunnelDiscoveryStage: failed to check tunnel_pending");
                false
            }
        }
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // 1. Get tunnel candidates from DB (bounded by batch size)
        let candidates = ctx.database.get_tunnel_candidates(TUNNEL_BATCH_SIZE).await?;

        if candidates.is_empty() {
            info!("TunnelDiscoveryStage: no tunnel candidates found");
            return Ok(ctx);
        }

        info!(
            candidate_count = candidates.len(),
            "TunnelDiscoveryStage: processing tunnel candidates"
        );

        // 2. Group by topic
        let topic_groups = group_by_topic(&candidates);

        let mut tunnels_created: usize = 0;

        for (topic, facts_in_topic) in &topic_groups {
            // 3. For each topic, group by domain
            let domain_groups = group_by_domain(facts_in_topic);

            // Only proceed if facts span 2+ domains
            if domain_groups.len() < 2 {
                // Clear pending flags even for single-domain topics
                ctx.database.clear_tunnel_pending_by_topic(topic).await?;
                continue;
            }

            // Pick representative fact per domain (highest strength)
            let representatives: Vec<(&str, &MemoryFact)> = domain_groups
                .iter()
                .filter_map(|(domain, facts)| {
                    pick_representative(facts).map(|rep| (domain.as_str(), rep))
                })
                .collect();

            // Compute pairwise embedding cosine similarity
            for i in 0..representatives.len() {
                for j in (i + 1)..representatives.len() {
                    let (domain_a, fact_a) = representatives[i];
                    let (domain_b, fact_b) = representatives[j];

                    let sim = embedding_similarity(
                        fact_a.embedding.as_deref(),
                        fact_b.embedding.as_deref(),
                    );

                    if sim >= TUNNEL_SIMILARITY_THRESHOLD {
                        // NOTE: Tunnel edges were previously stored in graph_nodes/graph_edges.
                        // That system has been replaced by Knowledge Notes. Tunnel discovery
                        // now only logs the relationship — wiring to notes_links is a future task.
                        tunnels_created += 1;
                        debug!(
                            topic = %topic,
                            domain_a = %domain_a,
                            domain_b = %domain_b,
                            similarity = sim,
                            "Created tunnel edge"
                        );
                    }
                }
            }

            // 4. Clear tunnel_pending flags for this topic
            ctx.database.clear_tunnel_pending_by_topic(topic).await?;
        }

        info!(
            tunnels_created = tunnels_created,
            topics_processed = topic_groups.len(),
            "TunnelDiscoveryStage completed"
        );

        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;

    fn make_fact(id: &str, path: &str, strength: f32, embedding: Option<Vec<f32>>) -> MemoryFact {
        let mut fact = MemoryFact::with_id(id.into(), "test content".into(), FactType::Other);
        fact.path = path.into();
        fact.strength = strength;
        fact.embedding = embedding;
        fact
    }

    #[test]
    fn group_by_topic_groups_correctly() {
        let facts = vec![
            make_fact("1", "aleph://user/preferences/a", 1.0, None),
            make_fact("2", "aleph://knowledge/preferences/b", 1.0, None),
            make_fact("3", "aleph://user/projects/c", 1.0, None),
        ];
        let groups = group_by_topic(&facts);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["preferences"].len(), 2);
        assert_eq!(groups["projects"].len(), 1);
    }

    #[test]
    fn group_by_domain_groups_correctly() {
        let f1 = make_fact("1", "aleph://user/preferences/a", 1.0, None);
        let f2 = make_fact("2", "aleph://knowledge/preferences/b", 1.0, None);
        let f3 = make_fact("3", "aleph://user/projects/c", 1.0, None);
        let refs: Vec<&MemoryFact> = vec![&f1, &f2, &f3];
        let groups = group_by_domain(&refs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["user"].len(), 2);
        assert_eq!(groups["knowledge"].len(), 1);
    }

    #[test]
    fn embedding_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((embedding_similarity(Some(&a), Some(&a)) - 1.0).abs() < 0.001);
    }

    #[test]
    fn embedding_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(embedding_similarity(Some(&a), Some(&b)).abs() < 0.001);
    }

    #[test]
    fn embedding_similarity_none_returns_zero() {
        assert_eq!(embedding_similarity(None, Some(&[1.0])), 0.0);
        assert_eq!(embedding_similarity(None, None), 0.0);
    }

    #[test]
    fn embedding_similarity_mismatched_lengths_returns_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(embedding_similarity(Some(&a), Some(&b)), 0.0);
    }

    #[test]
    fn embedding_similarity_empty_returns_zero() {
        let empty: Vec<f32> = vec![];
        let a = vec![1.0];
        assert_eq!(embedding_similarity(Some(&empty), Some(&a)), 0.0);
    }

    #[test]
    fn group_by_topic_skips_empty_paths() {
        let facts = vec![
            make_fact("1", "", 1.0, None),
            make_fact("2", "aleph://user/preferences/a", 1.0, None),
        ];
        let groups = group_by_topic(&facts);
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("preferences"));
    }

    #[test]
    fn group_by_domain_skips_empty_paths() {
        let f1 = make_fact("1", "", 1.0, None);
        let f2 = make_fact("2", "aleph://user/preferences/a", 1.0, None);
        let refs: Vec<&MemoryFact> = vec![&f1, &f2];
        let groups = group_by_domain(&refs);
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("user"));
    }
}

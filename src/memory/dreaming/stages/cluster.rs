//! ClusterStage: groups memories into semantic clusters.
//!
//! Two-phase approach:
//! 1. Metadata pre-grouping (by session or day) to reduce DBSCAN input size
//! 2. DBSCAN vector clustering within each group

use async_trait::async_trait;
use tracing::debug;

use crate::error::AlephError;
use crate::memory::context::MemoryEntry;
use super::{DreamStage, DreamContext};

/// Threshold: groups with >= this many memories use session-based pre-grouping
const PREGROUP_SESSION_THRESHOLD: usize = 200;

/// Threshold: groups with >= this many memories use day-based pre-grouping
const PREGROUP_DAY_THRESHOLD: usize = 50;

// ---------------------------------------------------------------------------
// MetadataGroupKey
// ---------------------------------------------------------------------------

/// Key for grouping memories by metadata before clustering.
#[derive(Debug, Clone)]
pub enum MetadataGroupKey {
    Session(String),
    TimeWindow { day: String },
    None,
}

// ---------------------------------------------------------------------------
// MemoryCluster
// ---------------------------------------------------------------------------

/// A cluster of related memories.
#[derive(Debug, Clone)]
pub struct MemoryCluster {
    pub id: String,
    pub label: String,
    pub members: Vec<MemoryEntry>,
    pub centroid: Option<Vec<f32>>,
    pub metadata_key: MetadataGroupKey,
    pub is_noise: bool,
}

// ---------------------------------------------------------------------------
// DBSCAN algorithm
// ---------------------------------------------------------------------------

/// Cosine distance: 1.0 - cosine_similarity.
/// Returns 1.0 when either vector has zero norm.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
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

/// Find all point indices within `eps` cosine distance of `points[idx]`.
fn range_query(points: &[Vec<f32>], idx: usize, eps: f32) -> Vec<usize> {
    let mut neighbors = Vec::new();
    for (i, point) in points.iter().enumerate() {
        if cosine_distance(&points[idx], point) <= eps {
            neighbors.push(i);
        }
    }
    neighbors
}

/// DBSCAN clustering on embedding vectors using cosine distance.
///
/// Returns a label per point: -1 = noise, 0.. = cluster id.
pub fn dbscan(points: &[Vec<f32>], eps: f32, min_samples: usize) -> Vec<i32> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }

    // Labels: -2 = undefined, -1 = noise, 0.. = cluster id
    let mut labels = vec![-2i32; n];
    let mut cluster_id: i32 = 0;

    for i in 0..n {
        if labels[i] != -2 {
            continue; // already processed
        }

        let neighbors = range_query(points, i, eps);

        if neighbors.len() < min_samples {
            labels[i] = -1; // noise
            continue;
        }

        // Start a new cluster
        labels[i] = cluster_id;

        // Seed set: neighbors minus point i itself
        let mut seed_set: Vec<usize> = neighbors.into_iter().filter(|&j| j != i).collect();
        let mut seed_idx = 0;

        while seed_idx < seed_set.len() {
            let q = seed_set[seed_idx];
            seed_idx += 1;

            if labels[q] == -1 {
                // Was noise, now border point
                labels[q] = cluster_id;
            }

            if labels[q] != -2 {
                continue; // already assigned to a cluster
            }

            labels[q] = cluster_id;

            let q_neighbors = range_query(points, q, eps);
            if q_neighbors.len() >= min_samples {
                for &neighbor in &q_neighbors {
                    if labels[neighbor] == -2 || labels[neighbor] == -1 {
                        if !seed_set.contains(&neighbor) {
                            seed_set.push(neighbor);
                        }
                    }
                }
            }
        }

        cluster_id += 1;
    }

    labels
}

// ---------------------------------------------------------------------------
// Metadata pre-grouping
// ---------------------------------------------------------------------------

/// Pre-group memories by metadata to reduce per-group DBSCAN input size.
///
/// - < 50 memories: no grouping (single group with `MetadataGroupKey::None`)
/// - 50..200 memories: group by calendar day
/// - > 200 memories: group by session_id
fn pregroup(memories: &[MemoryEntry]) -> Vec<(MetadataGroupKey, Vec<MemoryEntry>)> {
    let n = memories.len();

    if n < PREGROUP_DAY_THRESHOLD {
        return vec![(MetadataGroupKey::None, memories.to_vec())];
    }

    if n > PREGROUP_SESSION_THRESHOLD {
        // Group by session_id
        let mut groups: std::collections::HashMap<String, Vec<MemoryEntry>> =
            std::collections::HashMap::new();
        for m in memories {
            groups
                .entry(m.context.session_id.clone())
                .or_default()
                .push(m.clone());
        }
        return groups
            .into_iter()
            .map(|(sid, members)| (MetadataGroupKey::Session(sid), members))
            .collect();
    }

    // Group by day (YYYY-MM-DD from timestamp)
    let mut groups: std::collections::HashMap<String, Vec<MemoryEntry>> =
        std::collections::HashMap::new();
    for m in memories {
        let day = chrono::DateTime::from_timestamp(m.context.timestamp, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        groups.entry(day).or_default().push(m.clone());
    }
    groups
        .into_iter()
        .map(|(day, members)| (MetadataGroupKey::TimeWindow { day }, members))
        .collect()
}

// ---------------------------------------------------------------------------
// Centroid computation
// ---------------------------------------------------------------------------

/// Compute the mean embedding vector for a set of embeddings.
fn compute_centroid(embeddings: &[&Vec<f32>]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    if dim == 0 {
        return None;
    }
    let count = embeddings.len() as f32;
    let mut centroid = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, &val) in emb.iter().enumerate() {
            centroid[i] += val;
        }
    }
    for val in &mut centroid {
        *val /= count;
    }
    Some(centroid)
}

// ---------------------------------------------------------------------------
// ClusterStage
// ---------------------------------------------------------------------------

/// Groups collected memories into semantic clusters using DBSCAN.
pub struct ClusterStage;

#[async_trait]
impl DreamStage for ClusterStage {
    fn name(&self) -> &'static str {
        "cluster"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let eps = ctx.config.cluster_dbscan_eps();
        let min_samples = ctx.config.cluster_dbscan_min_samples();

        if ctx.memories.is_empty() {
            debug!("ClusterStage: no memories to cluster");
            return Ok(ctx);
        }

        // Phase 1: pre-group by metadata
        let groups = pregroup(&ctx.memories);
        debug!(
            groups = groups.len(),
            total_memories = ctx.memories.len(),
            "ClusterStage: pre-grouped memories"
        );

        let mut all_clusters: Vec<MemoryCluster> = Vec::new();
        let mut cluster_counter: usize = 0;

        // Phase 2: DBSCAN within each group
        for (group_key, members) in groups {
            // Separate memories with and without embeddings
            let with_emb: Vec<(usize, &Vec<f32>)> = members
                .iter()
                .enumerate()
                .filter_map(|(i, m)| m.embedding.as_ref().map(|e| (i, e)))
                .collect();

            if with_emb.is_empty() {
                // All members lack embeddings — treat as single noise cluster
                all_clusters.push(MemoryCluster {
                    id: format!("cluster-{cluster_counter}"),
                    label: "no-embedding".to_string(),
                    members,
                    centroid: None,
                    metadata_key: group_key,
                    is_noise: true,
                });
                cluster_counter += 1;
                continue;
            }

            // Build embedding matrix for DBSCAN
            let embeddings: Vec<Vec<f32>> =
                with_emb.iter().map(|(_, e)| (*e).clone()).collect();
            let emb_to_member: Vec<usize> = with_emb.iter().map(|(i, _)| *i).collect();

            let labels = dbscan(&embeddings, eps, min_samples);

            // Collect members without embeddings into noise
            let no_emb_indices: Vec<usize> = members
                .iter()
                .enumerate()
                .filter(|(_, m)| m.embedding.is_none())
                .map(|(i, _)| i)
                .collect();

            // Group by label
            let mut label_groups: std::collections::HashMap<i32, Vec<usize>> =
                std::collections::HashMap::new();
            for (emb_idx, &label) in labels.iter().enumerate() {
                label_groups
                    .entry(label)
                    .or_default()
                    .push(emb_to_member[emb_idx]);
            }

            // Add no-embedding members to noise group
            if !no_emb_indices.is_empty() {
                label_groups.entry(-1).or_default().extend(no_emb_indices);
            }

            // Build MemoryCluster structs
            for (label, member_indices) in label_groups {
                let is_noise = label == -1;
                let cluster_members: Vec<MemoryEntry> =
                    member_indices.iter().map(|&i| members[i].clone()).collect();

                // Compute centroid from members that have embeddings
                let member_embeddings: Vec<&Vec<f32>> = cluster_members
                    .iter()
                    .filter_map(|m| m.embedding.as_ref())
                    .collect();
                let centroid = compute_centroid(&member_embeddings);

                let cluster_label = if is_noise {
                    "noise".to_string()
                } else {
                    format!("cluster-{label}")
                };

                all_clusters.push(MemoryCluster {
                    id: format!("cluster-{cluster_counter}"),
                    label: cluster_label,
                    members: cluster_members,
                    centroid,
                    metadata_key: group_key.clone(),
                    is_noise,
                });
                cluster_counter += 1;
            }
        }

        debug!(
            clusters = all_clusters.len(),
            "ClusterStage: formed clusters"
        );

        ctx.clusters = all_clusters;
        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a 2D point as Vec<f32>.
    fn point(x: f32, y: f32) -> Vec<f32> {
        vec![x, y]
    }

    #[test]
    fn dbscan_two_obvious_clusters() {
        // Cluster A: points near (1, 0)
        // Cluster B: points near (0, 1)
        // Cosine distance between (1,0) and (0,1) = 1.0, well above eps=0.3
        let points = vec![
            point(1.0, 0.0),
            point(1.0, 0.1),
            point(1.0, 0.05),
            point(0.0, 1.0),
            point(0.1, 1.0),
            point(0.05, 1.0),
        ];
        let labels = dbscan(&points, 0.3, 2);
        assert_eq!(labels.len(), 6);

        // First three should share a label, last three should share a different label
        assert!(labels[0] >= 0);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[0], labels[2]);

        assert!(labels[3] >= 0);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[3], labels[5]);

        // The two clusters should have different labels
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn dbscan_all_noise() {
        // Points far apart in direction — cosine distance >> eps
        let points = vec![
            point(1.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, 0.0),
        ];
        let labels = dbscan(&points, 0.1, 2);
        assert_eq!(labels.len(), 3);
        // All should be noise
        assert!(labels.iter().all(|&l| l == -1));
    }

    #[test]
    fn dbscan_empty_input() {
        let points: Vec<Vec<f32>> = vec![];
        let labels = dbscan(&points, 0.3, 2);
        assert!(labels.is_empty());
    }

    #[test]
    fn dbscan_single_point() {
        let points = vec![point(1.0, 0.0)];
        // min_samples=2 so a single point cannot form a cluster
        let labels = dbscan(&points, 0.3, 2);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0], -1); // noise
    }

    #[test]
    fn dbscan_all_identical_points() {
        // All identical → cosine distance = 0.0 → all in one cluster
        let points = vec![
            point(1.0, 1.0),
            point(1.0, 1.0),
            point(1.0, 1.0),
            point(1.0, 1.0),
        ];
        let labels = dbscan(&points, 0.3, 2);
        assert_eq!(labels.len(), 4);
        // All should be in the same cluster (not noise)
        assert!(labels[0] >= 0);
        assert!(labels.iter().all(|&l| l == labels[0]));
    }

    #[test]
    fn dbscan_single_point_min_samples_one() {
        // With min_samples=1, even a lone point forms a cluster
        let points = vec![point(1.0, 0.0)];
        let labels = dbscan(&points, 0.3, 1);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0], 0);
    }

    #[test]
    fn cosine_distance_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let d = cosine_distance(&a, &b);
        assert!(d.abs() < 1e-5, "expected ~0.0, got {d}");
    }

    #[test]
    fn cosine_distance_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let d = cosine_distance(&a, &b);
        assert!((d - 1.0).abs() < 1e-5, "expected ~1.0, got {d}");
    }

    #[test]
    fn cosine_distance_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        let d = cosine_distance(&a, &b);
        assert!((d - 1.0).abs() < 1e-5, "expected 1.0 for zero vector");
    }

    #[test]
    fn pregroup_small_set() {
        let memories = make_test_memories(10, "sess-1", 1000);
        let groups = pregroup(&memories);
        assert_eq!(groups.len(), 1);
        assert!(matches!(groups[0].0, MetadataGroupKey::None));
        assert_eq!(groups[0].1.len(), 10);
    }

    #[test]
    fn pregroup_medium_groups_by_day() {
        // 60 memories across 2 days
        let mut memories = make_test_memories(30, "sess-1", 1_700_000_000); // day 1
        memories.extend(make_test_memories(30, "sess-1", 1_700_000_000 + 86400)); // day 2
        let groups = pregroup(&memories);
        assert!(groups.len() >= 2, "expected >=2 day groups, got {}", groups.len());
        for (key, _) in &groups {
            assert!(matches!(key, MetadataGroupKey::TimeWindow { .. }));
        }
    }

    #[test]
    fn pregroup_large_groups_by_session() {
        // 210 memories across 3 sessions
        let mut memories = make_test_memories(70, "sess-a", 1_700_000_000);
        memories.extend(make_test_memories(70, "sess-b", 1_700_000_000));
        memories.extend(make_test_memories(71, "sess-c", 1_700_000_000));
        let groups = pregroup(&memories);
        assert_eq!(groups.len(), 3);
        for (key, _) in &groups {
            assert!(matches!(key, MetadataGroupKey::Session(_)));
        }
    }

    /// Helper: create N test MemoryEntry values with given session_id and base timestamp.
    fn make_test_memories(n: usize, session_id: &str, base_ts: i64) -> Vec<MemoryEntry> {
        use crate::memory::context::ContextAnchor;
        (0..n)
            .map(|i| {
                let ctx = ContextAnchor {
                    window_title: format!("window-{i}"),
                    timestamp: base_ts + (i as i64 * 60),
                    session_id: session_id.to_string(),
                };
                MemoryEntry {
                    id: format!("mem-{session_id}-{i}"),
                    context: ctx,
                    user_input: format!("input {i}"),
                    ai_output: format!("output {i}"),
                    embedding: None,
                    namespace: "owner".to_string(),
                    agent: "main".to_string(),
                    similarity_score: None,
                }
            })
            .collect()
    }
}

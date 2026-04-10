//! Shared data types and utilities for dream pipeline stages.

use crate::memory::context::MemoryEntry;

/// Key for grouping memories by metadata before clustering.
#[derive(Debug, Clone)]
pub enum MetadataGroupKey {
    Session(String),
    TimeWindow { day: String },
    None,
}

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
// DBSCAN algorithm (used by DeepSynthesisStage)
// ---------------------------------------------------------------------------

/// Cosine distance between two embedding vectors. Returns 1.0 on zero norm.
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

/// Find indices of all points within `eps` cosine distance of `points[idx]`.
fn range_query(points: &[Vec<f32>], idx: usize, eps: f32) -> Vec<usize> {
    points
        .iter()
        .enumerate()
        .filter(|(_, p)| cosine_distance(&points[idx], p) <= eps)
        .map(|(i, _)| i)
        .collect()
}

/// DBSCAN clustering on embedding vectors using cosine distance.
///
/// Returns a label vector where -1 means noise and ≥0 is a cluster id.
pub fn dbscan(points: &[Vec<f32>], eps: f32, min_pts: usize) -> Vec<i32> {
    let n = points.len();
    let mut labels = vec![-1i32; n];
    let mut cluster_id: i32 = 0;

    for i in 0..n {
        if labels[i] != -1 {
            continue;
        }
        let mut neighbors = range_query(points, i, eps);
        if neighbors.len() < min_pts {
            // Mark as noise; may be reassigned later
            labels[i] = -1;
            continue;
        }
        labels[i] = cluster_id;
        let mut seed_idx = 0;
        while seed_idx < neighbors.len() {
            let q = neighbors[seed_idx];
            if labels[q] == -1 {
                labels[q] = cluster_id;
            }
            if labels[q] != -1 && labels[q] != cluster_id {
                seed_idx += 1;
                continue;
            }
            labels[q] = cluster_id;
            let q_neighbors = range_query(points, q, eps);
            if q_neighbors.len() >= min_pts {
                neighbors.extend(q_neighbors.into_iter().filter(|&nb| labels[nb] == -1));
            }
            seed_idx += 1;
        }
        cluster_id += 1;
    }

    labels
}

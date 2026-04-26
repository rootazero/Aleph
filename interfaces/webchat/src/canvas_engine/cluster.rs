use crate::canvas_engine::types::*;
use std::collections::HashMap;

pub const FOLD_THRESHOLD: usize = 12;

/// Fold neighbors of a single relation sector into ClusterNodes,
/// grouping by `CanvasNode::category` into `ClusterNode::kind` buckets.
/// Returns (unfolded_nodes, clusters).
// NOTE: no longer called from adapter; Task 3 will delete this.
#[allow(dead_code)]
pub fn fold_sector(
    neighbors: &[CanvasNode],
    relation: &str,
    active_id: &str,
    threshold: usize,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    let mut by_category: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for n in neighbors {
        by_category.entry(n.category.clone()).or_default().push(n.clone());
    }

    let mut unfolded = Vec::new();
    let mut clusters = Vec::new();
    for (category, mut group) in by_category {
        if group.len() >= threshold {
            // Sort by descending weight for representative picking
            group.sort_by(|a, b| {
                let wa = a.decay_score * a.edge_count as f32;
                let wb = b.decay_score * b.edge_count as f32;
                wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
            });
            let aggregated_weight: f32 = group
                .iter()
                .map(|n| n.decay_score * n.edge_count as f32)
                .sum();
            let representative_names: Vec<String> =
                group.iter().take(3).map(|n| n.name.clone()).collect();
            let member_ids: Vec<String> = group.iter().map(|n| n.id.clone()).collect();
            let radius = cluster_radius(member_ids.len());
            clusters.push(ClusterNode {
                id: format!("cluster::{}::{}::{}", relation, category, active_id),
                relation: relation.to_string(),
                kind: category,
                member_ids,
                representative_names,
                aggregated_weight,
                radius,
                world_pos: Vec2::new(0.0, 0.0),
                z: 60.0,
                expanded: false,
            });
        } else {
            unfolded.extend(group);
        }
    }
    // Sort clusters by kind for deterministic order — downstream sector-angle
    // assignment (Tasks 6/11) depends on stable iteration across page loads.
    clusters.sort_by(|a, b| a.kind.cmp(&b.kind));
    (unfolded, clusters)
}

/// Fallback fold: if total neighbors >= 30 and no individual kind hits threshold,
/// keep top 20 by weight and force-fold the rest.
pub fn fallback_fold(
    mut neighbors: Vec<CanvasNode>,
    relation: &str,
    active_id: &str,
) -> (Vec<CanvasNode>, Vec<ClusterNode>) {
    if neighbors.len() < 30 {
        return (neighbors, vec![]);
    }
    neighbors.sort_by(|a, b| {
        let wa = a.decay_score * a.edge_count as f32;
        let wb = b.decay_score * b.edge_count as f32;
        wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let kept: Vec<_> = neighbors.drain(..20).collect();
    // Pass singletons through as unfolded; only fold groups of 2+.
    let (mut singletons, clusters) = fold_sector(&neighbors, relation, active_id, 2);
    let mut result = kept;
    result.append(&mut singletons);
    (result, clusters)
}

/// Fold a slice of nodes into one ClusterNode per distinct `CanvasNode::category`.
/// Each cluster's `representative_names` is the top 3 by descending weight.
///
/// `relation` is set to "_default" since the underlying graph edges no longer
/// carry a relation field.
pub fn group_by_category_into_clusters(
    nodes: Vec<CanvasNode>,
    active_id: &str,
) -> Vec<ClusterNode> {
    let mut by_category: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for n in nodes {
        by_category.entry(n.category.clone()).or_default().push(n);
    }

    let mut clusters: Vec<ClusterNode> = Vec::with_capacity(by_category.len());
    for (category, mut group) in by_category {
        group.sort_by(|a, b| {
            let wa = a.decay_score * a.edge_count as f32;
            let wb = b.decay_score * b.edge_count as f32;
            match wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal) {
                std::cmp::Ordering::Equal => a.id.cmp(&b.id),
                other => other,
            }
        });
        let aggregated_weight: f32 =
            group.iter().map(|n| n.decay_score * n.edge_count as f32).sum();
        let representative_names: Vec<String> =
            group.iter().take(3).map(|n| n.name.clone()).collect();
        let member_ids: Vec<String> = group.iter().map(|n| n.id.clone()).collect();
        let radius = cluster_radius(member_ids.len());
        clusters.push(ClusterNode {
            id: format!("cluster::_default::{}::{}", category, active_id),
            relation: "_default".to_string(),
            kind: category,
            member_ids,
            representative_names,
            aggregated_weight,
            radius,
            world_pos: Vec2::new(0.0, 0.0),
            z: 60.0,
            expanded: false,
        });
    }
    clusters.sort_by(|a, b| a.kind.cmp(&b.kind));
    clusters
}

/// Compute ClusterNode display radius: 24 + 6 * log2(N), capped at 60.
pub fn cluster_radius(n: usize) -> f32 {
    let r = 24.0 + 6.0 * (n.max(2) as f32).log2();
    r.min(60.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_node(id: &str, category: &str, weight: f32) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            name: id.to_string(),
            category: category.to_string(),
            color: Color::new(0, 0, 0),
            radius: 30.0,
            position: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            pinned: false,
            z: 60.0,
            hop: 1,
            decay_score: weight,
            edge_count: 1,
        }
    }

    #[test]
    fn fold_below_threshold_keeps_all() {
        let neighbors: Vec<_> = (0..11)
            .map(|i| mock_node(&format!("n{i}"), "concept", 1.0))
            .collect();
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 11);
        assert_eq!(clusters.len(), 0);
    }

    #[test]
    fn fold_at_threshold_creates_cluster() {
        let neighbors: Vec<_> = (0..12)
            .map(|i| mock_node(&format!("n{i}"), "concept", 1.0))
            .collect();
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 0);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_ids.len(), 12);
        assert_eq!(clusters[0].kind, "concept");
        assert_eq!(clusters[0].id, "cluster::uses::concept::active");
    }

    #[test]
    fn fold_mixed_kinds_only_folds_qualifying_groups() {
        let mut neighbors: Vec<_> = (0..12)
            .map(|i| mock_node(&format!("c{i}"), "concept", 1.0))
            .collect();
        neighbors.extend((0..5).map(|i| mock_node(&format!("p{i}"), "person", 1.0)));
        let (unfolded, clusters) = fold_sector(&neighbors, "uses", "active", FOLD_THRESHOLD);
        assert_eq!(unfolded.len(), 5, "person nodes should remain unfolded");
        assert_eq!(clusters.len(), 1, "concept group should fold");
    }

    #[test]
    fn cluster_radius_log_scaling() {
        assert!((cluster_radius(2) - (24.0 + 6.0)).abs() < 1e-3); // log2(2) = 1
        assert!((cluster_radius(16) - (24.0 + 24.0)).abs() < 1e-3); // log2(16) = 4
        assert!(cluster_radius(1024) <= 60.0); // capped
    }

    #[test]
    fn fallback_fold_triggers_at_30() {
        let neighbors: Vec<_> = (0..30)
            .map(|i| {
                let cat = if i < 11 {
                    "concept"
                } else if i < 22 {
                    "person"
                } else {
                    "tool"
                };
                mock_node(&format!("n{i}"), cat, i as f32)
            })
            .collect();
        let (unfolded, clusters) = fallback_fold(neighbors, "uses", "active");
        assert_eq!(unfolded.len(), 20, "top 20 by weight kept");
        assert!(!clusters.is_empty(), "remainder force-folded");
    }

    #[test]
    fn group_by_category_creates_one_cluster_per_category() {
        let nodes = vec![
            node("a", "concept"),
            node("b", "concept"),
            node("c", "reference"),
        ];
        let clusters = group_by_category_into_clusters(nodes, "center");
        assert_eq!(clusters.len(), 2, "concept + reference => 2 clusters");
        let kinds: Vec<&str> = clusters.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"concept"));
        assert!(kinds.contains(&"reference"));
    }

    #[test]
    fn group_by_category_uses_top3_names_as_representatives() {
        let nodes: Vec<CanvasNode> = (0..5)
            .map(|i| {
                let mut n = node(&format!("n{i}"), "concept");
                n.name = format!("name-{i}");
                n.decay_score = (5 - i) as f32; // n0 highest, n4 lowest
                n.edge_count = 1;
                n
            })
            .collect();
        let clusters = group_by_category_into_clusters(nodes, "center");
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].representative_names, vec!["name-0", "name-1", "name-2"]);
    }

    // Helper used by the new tests
    fn node(id: &str, category: &str) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            name: id.to_string(),
            category: category.to_string(),
            color: Color::new(0, 0, 0),
            radius: 24.0,
            position: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            z: 0.0,
            hop: 1,
            pinned: false,
            decay_score: 1.0,
            edge_count: 1,
        }
    }

    #[test]
    fn fallback_fold_singletons_stay_unfolded() {
        // 30 nodes total. Node 0 (weight 0) is the unique "tool"; nodes 1..30 are
        // "concept" with weights 1..29. After sort-by-weight-desc and drain(..20),
        // the kept top 20 are weights 10..29 (all concepts). The remainder is
        // weights 0..9: 9 concepts + 1 tool. Under threshold=2, the lone tool
        // must NOT be force-folded into a 1-member cluster.
        let neighbors: Vec<_> = (0..30)
            .map(|i| {
                let cat = if i == 0 { "tool" } else { "concept" };
                mock_node(&format!("n{i}"), cat, i as f32)
            })
            .collect();
        let (unfolded, clusters) = fallback_fold(neighbors, "uses", "active");
        assert_eq!(unfolded.len(), 21, "20 top-weight + 1 singleton from remainder");
        assert!(
            clusters.iter().all(|c| c.member_ids.len() >= 2),
            "no 1-member clusters allowed"
        );
        // The lone tool (id "n0") must appear among unfolded, not inside a cluster.
        assert!(unfolded.iter().any(|n| n.id == "n0"));
        assert!(clusters
            .iter()
            .all(|c| !c.member_ids.iter().any(|id| id == "n0")));
    }
}

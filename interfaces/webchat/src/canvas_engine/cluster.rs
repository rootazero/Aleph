use crate::canvas_engine::types::*;
use std::collections::HashMap;

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
        let aggregated_weight: f32 = group
            .iter()
            .map(|n| n.decay_score * n.edge_count as f32)
            .sum();
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

    #[test]
    fn cluster_radius_log_scaling() {
        assert!((cluster_radius(2) - (24.0 + 6.0)).abs() < 1e-3); // log2(2) = 1
        assert!((cluster_radius(16) - (24.0 + 24.0)).abs() < 1e-3); // log2(16) = 4
        assert!(cluster_radius(1024) <= 60.0); // capped
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
        assert_eq!(
            clusters[0].representative_names,
            vec!["name-0", "name-1", "name-2"]
        );
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
            decay_score: 1.0,
            edge_count: 1,
        }
    }
}

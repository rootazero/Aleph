//! Edge level-of-detail: which edges survive at a given density. Pure, so it is
//! unit-tested on the native target.
//!
//! This is what the sidebar's density slider actually drives (slider → `lod` via
//! `galaxy_build::fold_to_lod` → here). It lived inline in `Scene` and had no
//! tests at all, which is a bad place for a percentile index to hide.

use super::GraphData;

/// The edges to draw at density `lod`, with their kind and brightness vecs kept
/// in lockstep (they are parallel arrays — a filter that desynchronises them
/// mistints every edge after the first drop).
pub struct FilteredEdges {
    pub edges: Vec<(u32, u32)>,
    pub kinds: Vec<u8>,
    pub bright: Vec<f32>,
}

/// Filter `data`'s edges by a link-count floor derived from `lod`.
///
/// `lod` runs 0 → 1, sparse → dense inverted: **0 draws every edge**, 1 keeps only
/// the ~90th-percentile backbone. The floor is the `lod * 0.9` quantile of the
/// nodes' link counts.
///
/// An edge survives when EITHER endpoint clears the floor, not both: a weak note
/// hanging off a strong hub is exactly the spoke you want to keep, and requiring
/// both ends would shred the clusters into disconnected cores.
pub fn filter(data: &GraphData, lod: f32) -> FilteredEdges {
    // Kind/brightness default when the parallel vec is absent or short (legacy
    // data): wikilink, full brightness.
    let kind_at = |i: usize| data.edge_kinds.get(i).copied().unwrap_or(0);
    let bright_at = |i: usize| data.edge_bright.get(i).copied().unwrap_or(1.0);
    let all = || FilteredEdges {
        edges: data.edges.clone(),
        kinds: (0..data.edges.len()).map(kind_at).collect(),
        bright: (0..data.edges.len()).map(bright_at).collect(),
    };

    if lod <= 0.0 || data.nodes.is_empty() {
        return all();
    }

    let mut counts: Vec<u32> = data.nodes.iter().map(|n| n.link_count).collect();
    counts.sort_unstable();
    let last = counts.len().saturating_sub(1);
    let idx = ((lod * 0.9 * last as f32) as usize).min(last);
    let floor = counts[idx];

    // A zero floor excludes nothing, so skip the walk.
    if floor == 0 {
        return all();
    }

    let mut out = FilteredEdges {
        edges: Vec::new(),
        kinds: Vec::new(),
        bright: Vec::new(),
    };
    for (i, &(a, b)) in data.edges.iter().enumerate() {
        let lc_a = data.nodes.get(a as usize).map_or(0, |n| n.link_count);
        let lc_b = data.nodes.get(b as usize).map_or(0, |n| n.link_count);
        if lc_a >= floor || lc_b >= floor {
            out.edges.push((a, b));
            out.kinds.push(kind_at(i));
            out.bright.push(bright_at(i));
        }
    }
    debug_assert_eq!(out.edges.len(), out.kinds.len());
    debug_assert_eq!(out.edges.len(), out.bright.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::memory::galaxy::gl::math::Vec3;
    use crate::views::memory::galaxy::gl::GalaxyNode;

    /// A graph with explicit per-node link counts and explicit edges. The counts
    /// are what the floor is computed from, so tests state them outright rather
    /// than inferring them from a shape.
    fn graph(link_counts: &[u32], edges: &[(u32, u32)]) -> GraphData {
        let nodes = link_counts
            .iter()
            .enumerate()
            .map(|(i, &link_count)| GalaxyNode {
                id: format!("n{i}"),
                name: format!("n{i}"),
                category: "c".into(),
                link_count,
                pos: Vec3::zero(),
                color: [1.0, 1.0, 1.0],
                community: None,
            })
            .collect();
        GraphData {
            nodes,
            edges: edges.to_vec(),
            edge_kinds: (0..edges.len()).map(|i| (i % 8) as u8).collect(),
            edge_bright: (0..edges.len()).map(|i| 1.0 - i as f32 * 0.01).collect(),
        }
    }

    #[test]
    fn lod_zero_keeps_every_edge() {
        let g = graph(&[4, 1, 1, 1, 1], &[(0, 1), (0, 2), (0, 3), (0, 4)]);
        let f = filter(&g, 0.0);
        assert_eq!(f.edges, g.edges);
        assert_eq!(f.kinds, g.edge_kinds);
        assert_eq!(f.bright, g.edge_bright);
    }

    #[test]
    fn parallel_vecs_stay_in_lockstep_when_edges_are_dropped() {
        // Counts sorted: [1,1,1,1,1,1,1,1,5,9]; at lod=1 the index is
        // (0.9 * 9) = 8 → floor 5. So only edges touching node 8 (5) or 9 (9)
        // survive; the weak-to-weak chord (1,2) is culled — and its kind and
        // brightness must be culled WITH it, or every surviving edge renders in
        // its neighbour's colour.
        let g = graph(
            &[1, 1, 1, 1, 1, 1, 1, 1, 5, 9],
            &[(9, 0), (1, 2), (8, 3)], // hub-spoke, weak-weak chord, mid-spoke
        );
        let f = filter(&g, 1.0);

        assert_eq!(f.edges, vec![(9, 0), (8, 3)], "weak-to-weak chord culled");
        assert_eq!(f.edges.len(), f.kinds.len());
        assert_eq!(f.edges.len(), f.bright.len());
        // Each survivor keeps ITS OWN kind/brightness, not the dropped edge's:
        // (9,0) was source index 0 (kind 0, bright 1.00) and (8,3) index 2
        // (kind 2, bright 0.98). A naive filter that culled `edges` but not the
        // parallel vecs would hand (8,3) the chord's kind 1 / bright 0.99.
        assert_eq!(f.kinds, vec![0, 2]);
        assert!((f.bright[0] - 1.0).abs() < 1e-6);
        assert!((f.bright[1] - 0.98).abs() < 1e-6);
    }

    #[test]
    fn a_uniform_graph_is_never_shredded() {
        // Every node has the same link count, so the floor lands ON that count and
        // every node clears it. Density must not be able to blank a regular graph
        // (this is why the floor is a percentile of the counts, not a fixed cut).
        let g = graph(&[2, 2, 2, 2], &[(0, 1), (1, 2), (2, 3), (3, 0)]);
        for lod in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert_eq!(filter(&g, lod).edges.len(), 4, "lod={lod} dropped an edge");
        }
    }

    #[test]
    fn empty_graph_is_handled_at_every_density() {
        let g = GraphData::default();
        for lod in [0.0, 0.5, 1.0] {
            assert!(filter(&g, lod).edges.is_empty());
        }
    }

    #[test]
    fn density_is_monotonic() {
        // Turning the slider toward "sparse" must never ADD edges back. A graded
        // spread of link counts so the floor actually climbs as lod rises.
        let counts_per_node: Vec<u32> = (0..12).map(|i| i as u32).collect();
        let edges: Vec<(u32, u32)> = (0..11).map(|i| (i, i + 1)).collect();
        let g = graph(&counts_per_node, &edges);

        let drawn: Vec<usize> = (0..=10)
            .map(|k| filter(&g, k as f32 / 10.0).edges.len())
            .collect();
        assert!(
            drawn.windows(2).all(|w| w[0] >= w[1]),
            "edge count must be non-increasing in lod, got {drawn:?}"
        );
        assert!(
            drawn[0] > *drawn.last().unwrap(),
            "the slider must actually do something, got {drawn:?}"
        );
    }
}

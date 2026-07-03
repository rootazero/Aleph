//! Pure transforms that turn a `graph.query` response into renderable galaxy
//! data. Extracted from the `GalaxyCanvasView` component (`mod.rs`) so the
//! component file holds only reactive wiring (Effects + view). Everything here
//! is deterministic and unit-tested on the native target.

use super::gl;
use crate::canvas_engine::adapter::GraphQueryResponse;

/// Minimum brightness scale for the oldest note — keeps stale nodes visible
/// while newer ones glow brighter.
const RECENCY_FLOOR: f32 = 0.55;

/// Map a note's `updated_at` to a brightness scale in `[RECENCY_FLOOR, 1.0]`
/// across the graph's [oldest, newest] window. `None` (or a degenerate window)
/// → 1.0 (full brightness, no penalty).
fn recency_scale(updated_at: Option<i64>, oldest: i64, newest: i64) -> f32 {
    let Some(t) = updated_at else {
        return 1.0;
    };
    if newest <= oldest {
        return 1.0;
    }
    let f = ((t - oldest) as f32 / (newest - oldest) as f32).clamp(0.0, 1.0);
    RECENCY_FLOOR + (1.0 - RECENCY_FLOOR) * f
}

/// Build the initial 3D galaxy GraphData from a full-graph query response.
///
/// Node positions come from `ForceLayout::seed` (deterministic, hash-derived)
/// so the scene's starting positions match what the layout engine expects.
/// Scene::set_graph then builds a ForceLayout over these positions and animates
/// them to their settled state over up to MAX_SETTLE_STEPS frames.
pub(super) fn build_galaxy(resp: &GraphQueryResponse) -> gl::GraphData {
    use crate::canvas_engine::category_color::category_rgb;
    use gl::layout3d::ForceLayout;
    use gl::{GalaxyNode, GraphData};

    let mut id_index = std::collections::HashMap::new();
    for (i, n) in resp.nodes.iter().enumerate() {
        id_index.insert(n.id.clone(), i as u32);
    }

    // Memory links are directed rows, but the galaxy is an undirected graph:
    // reciprocal wikilinks (A→B and B→A) and duplicate rows must collapse to a
    // single edge, or each pair draws two oppositely-bowed bézier arcs (the
    // "double arc" artifact). Also drops self-loops.
    let (edges, edge_kinds) = dedup_undirected_edges(resp.edges.iter().filter_map(|e| {
        Some((
            *id_index.get(&e.from)?,
            *id_index.get(&e.to)?,
            gl::edges::edge_kind_code(e.kind.as_deref()),
        ))
    }));

    let ids: Vec<String> = resp.nodes.iter().map(|n| n.id.clone()).collect();
    let communities: Vec<Option<u32>> = resp.nodes.iter().map(|n| n.community_id).collect();
    let layout = ForceLayout::new(ids.len(), &edges, &communities);
    let positions = layout.seed(&ids);

    // Recency window across the returned nodes (for brightness scaling).
    let (oldest, newest) = resp
        .nodes
        .iter()
        .filter_map(|n| n.updated_at)
        .fold((i64::MAX, i64::MIN), |(lo, hi), t| (lo.min(t), hi.max(t)));

    let nodes: Vec<GalaxyNode> = resp
        .nodes
        .iter()
        .zip(positions)
        .map(|(n, pos)| {
            let scale = recency_scale(n.updated_at, oldest, newest);
            let base = category_rgb(&n.category);
            GalaxyNode {
                id: n.id.clone(),
                name: n.name.clone(),
                category: n.category.clone(),
                link_count: n.link_count as u32,
                pos,
                color: [base[0] * scale, base[1] * scale, base[2] * scale],
                community: n.community_id,
            }
        })
        .collect();

    GraphData {
        nodes,
        edges,
        edge_kinds,
    }
}

/// Collapse directed link rows into unique undirected edges, carrying each
/// edge's relation-kind code.
///
/// Reciprocal links (`A→B` and `B→A`) and exact duplicates fold to one
/// `(min, max)` pair; self-loops (`A→A`) are dropped. First appearance wins —
/// both the edge order and its kind — so rebuilds stay deterministic.
/// Normalizing to `(min, max)` also matches the edge-highlight key normalization
/// in `gl::edges::EdgeRenderer::set_highlight`. Returns parallel `(edges, kinds)`.
fn dedup_undirected_edges(
    directed: impl Iterator<Item = (u32, u32, u8)>,
) -> (Vec<(u32, u32)>, Vec<u8>) {
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    let mut kinds = Vec::new();
    for (a, b, kind) in directed {
        if a == b {
            continue; // degenerate self-loop
        }
        let key = (a.min(b), a.max(b));
        if seen.insert(key) {
            edges.push(key);
            kinds.push(kind);
        }
    }
    (edges, kinds)
}

/// Map the Fold slider value (UI range 0..=10) to an edge-density LOD in [0,1]
/// for the galaxy renderer. Higher slider = denser graph: `fold=0` → lod 1.0
/// (only the ~90th-percentile backbone survives `Scene::recompute_filtered_edges`),
/// `fold=10` → lod 0.0 (all edges). The full slider travel spans the full LOD
/// range, replacing the old `1.0 - (ft-1)/999` map whose 0..10 input only
/// produced lod∈[0.991,1.0] (visibly no change).
pub(super) fn fold_to_lod(fold: usize) -> f32 {
    let ft = fold.min(10) as f32;
    (1.0 - ft / 10.0).clamp(0.0, 1.0)
}

/// Compute the highlight set for a selected node: the selected node's index
/// plus all topologically adjacent node indices (one hop).
///
/// Returns a `HashSet<u32>` of node indices (matching `GraphData.nodes` order).
/// The scene's `set_highlight` will dim any node NOT in this set.
pub(super) fn compute_highlight_set(
    data: &gl::GraphData,
    selected_id: &str,
) -> std::collections::HashSet<u32> {
    // Find the selected node's index.
    let Some(sel_idx) = data.nodes.iter().position(|n| n.id == selected_id) else {
        return std::collections::HashSet::new();
    };
    let sel_idx = sel_idx as u32;

    // Collect direct neighbors via edges.
    let mut hl = std::collections::HashSet::new();
    hl.insert(sel_idx);
    for &(a, b) in &data.edges {
        if a == sel_idx {
            hl.insert(b);
        } else if b == sel_idx {
            hl.insert(a);
        }
    }
    hl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_collapses_reciprocal_and_duplicate_edges() {
        // (0,1) & (1,0) same undirected; (2,3) twice. Kind of first occurrence wins.
        let directed = [(0u32, 1u32, 1u8), (1, 0, 2), (2, 3, 0), (2, 3, 3), (3, 4, 4)];
        let (edges, kinds) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1), (2, 3), (3, 4)]);
        assert_eq!(kinds, vec![1, 0, 4]); // first-seen kind per undirected edge
    }

    #[test]
    fn dedup_drops_self_loops() {
        let directed = [(5u32, 5u32, 1u8), (0, 1, 2)];
        let (edges, kinds) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1)]);
        assert_eq!(kinds, vec![2]);
    }

    #[test]
    fn fold_to_lod_spans_full_visible_range() {
        // Full slider travel (0..=10) must cover the full LOD range so the
        // control is visibly effective (the old 0..10→[0.991,1.0] map did not).
        assert_eq!(fold_to_lod(0), 1.0); // sparsest: backbone only
        assert_eq!(fold_to_lod(10), 0.0); // densest: all edges
        assert_eq!(fold_to_lod(5), 0.5); // midpoint
                                         // Monotonic decreasing: higher slider = denser graph (lower lod).
        assert!(fold_to_lod(2) > fold_to_lod(8));
        // Out-of-range slider values clamp instead of overflowing the LOD range.
        assert_eq!(fold_to_lod(99), 0.0);
    }

    #[test]
    fn recency_scale_maps_newest_to_full_oldest_to_floor() {
        // None → full brightness.
        assert_eq!(recency_scale(None, 100, 200), 1.0);
        // Degenerate window → full (avoid div-by-zero).
        assert_eq!(recency_scale(Some(150), 200, 200), 1.0);
        // Newest → 1.0, oldest → floor.
        assert!((recency_scale(Some(200), 100, 200) - 1.0).abs() < 1e-6);
        assert!((recency_scale(Some(100), 100, 200) - RECENCY_FLOOR).abs() < 1e-6);
        // Midpoint sits strictly between floor and 1.0.
        let mid = recency_scale(Some(150), 100, 200);
        assert!(mid > RECENCY_FLOOR && mid < 1.0);
        // Out-of-range clamps to floor.
        assert_eq!(recency_scale(Some(50), 100, 200), RECENCY_FLOOR);
    }
}

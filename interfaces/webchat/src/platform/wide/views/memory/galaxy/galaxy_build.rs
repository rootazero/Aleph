//! Pure transforms that turn a `graph.query` response into renderable galaxy
//! data. Extracted from the `GalaxyCanvasView` component (`mod.rs`) so the
//! component file holds only reactive wiring (Effects + view). Everything here
//! is deterministic and unit-tested on the native target.

use super::gl;
use crate::memory_graph::adapter::GraphQueryResponse;

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
    use crate::memory_graph::category_color::category_rgb;
    use gl::layout3d::ForceLayout;
    use gl::{GalaxyNode, GraphData};

    let mut id_index = std::collections::HashMap::new();
    for (i, n) in resp.nodes.iter().enumerate() {
        id_index.insert(n.id.clone(), i as u32);
    }

    // Surprising-edge pairs (cross-community insight highlights), normalized to
    // (min,max) index pairs so lookup matches the dedup key below. Endpoints
    // not present in this response's node set are silently dropped.
    let surprising: std::collections::HashSet<(u32, u32)> = resp
        .surprising_edges
        .iter()
        .filter_map(|(from, to)| {
            let a = *id_index.get(from)?;
            let b = *id_index.get(to)?;
            Some((a.min(b), a.max(b)))
        })
        .collect();

    // Bridge-node ids (graph-health cut vertices), boosted in the node color pass below.
    let bridge_set: std::collections::HashSet<&str> =
        resp.bridge_nodes.iter().map(String::as_str).collect();

    // Memory links are directed rows, but the galaxy is an undirected graph:
    // reciprocal wikilinks (A→B and B→A) and duplicate rows must collapse to a
    // single edge, or each pair draws two oppositely-bowed bézier arcs (the
    // "double arc" artifact). Also drops self-loops.
    let (edges, edge_kinds, edge_bright) =
        dedup_undirected_edges(resp.edges.iter().filter_map(|e| {
            let a = *id_index.get(&e.from)?;
            let b = *id_index.get(&e.to)?;
            let mut kind = gl::edges::edge_kind_code(e.kind.as_deref());
            if surprising.contains(&(a.min(b), a.max(b))) {
                kind = 7; // insight emphasis overrides the base kind
            }
            Some((a, b, kind, edge_brightness(e.confidence, kind)))
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
            // Bridge nodes (graph-health cut vertices) get a color boost so
            // they read as slightly brighter hubs in the bloom pass.
            let bridge_boost = if bridge_set.contains(n.id.as_str()) {
                1.25
            } else {
                1.0
            };
            let boost = scale * bridge_boost;
            GalaxyNode {
                id: n.id.clone(),
                name: n.name.clone(),
                category: n.category.clone(),
                link_count: n.link_count as u32,
                pos,
                color: [base[0] * boost, base[1] * boost, base[2] * boost],
                community: n.community_id,
            }
        })
        .collect();

    GraphData {
        nodes,
        edges,
        edge_kinds,
        edge_bright,
    }
}

/// Collapse directed link rows into unique undirected edges, carrying each
/// edge's relation-kind code and brightness.
///
/// Reciprocal links (`A→B` and `B→A`) and exact duplicates fold to one
/// `(min, max)` pair; self-loops (`A→A`) are dropped. First appearance wins —
/// the edge order, its kind, and its brightness — so rebuilds stay deterministic.
/// Normalizing to `(min, max)` also matches the edge-highlight key normalization
/// in `gl::edges::EdgeRenderer::set_highlight`. Returns parallel
/// `(edges, kinds, brightness)`.
fn dedup_undirected_edges(
    directed: impl Iterator<Item = (u32, u32, u8, f32)>,
) -> (Vec<(u32, u32)>, Vec<u8>, Vec<f32>) {
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    let mut kinds = Vec::new();
    let mut brights = Vec::new();
    for (a, b, kind, bright) in directed {
        if a == b {
            continue; // degenerate self-loop
        }
        let key = (a.min(b), a.max(b));
        if seen.insert(key) {
            edges.push(key);
            kinds.push(kind);
            brights.push(bright);
        }
    }
    (edges, kinds, brights)
}

/// Per-edge brightness: confidence dims the backbone (floor 0.55 keeps weak
/// links visible); mention/similarity kinds are fixed-dim; surprising (7) is
/// full — its >1.0 color already carries the bloom glow.
fn edge_brightness(confidence: Option<f32>, kind: u8) -> f32 {
    match kind {
        5 => 0.5,
        6 => 0.55,
        7 => 1.0,
        _ => confidence.map_or(1.0, |c| 0.55 + 0.45 * c.clamp(0.0, 1.0)),
    }
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
        let directed = [
            (0u32, 1u32, 1u8, 1.0f32),
            (1, 0, 2, 1.0),
            (2, 3, 0, 1.0),
            (2, 3, 3, 1.0),
            (3, 4, 4, 1.0),
        ];
        let (edges, kinds, _bright) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1), (2, 3), (3, 4)]);
        assert_eq!(kinds, vec![1, 0, 4]); // first-seen kind per undirected edge
    }

    #[test]
    fn dedup_drops_self_loops() {
        let directed = [(5u32, 5u32, 1u8, 1.0f32), (0, 1, 2, 1.0)];
        let (edges, kinds, _bright) = dedup_undirected_edges(directed.into_iter());
        assert_eq!(edges, vec![(0, 1)]);
        assert_eq!(kinds, vec![2]);
    }

    #[test]
    fn edge_brightness_maps_confidence_and_kinds() {
        assert!((edge_brightness(Some(1.0), 0) - 1.0).abs() < 1e-6);
        assert!((edge_brightness(Some(0.35), 0) - (0.55 + 0.45 * 0.35)).abs() < 1e-6);
        assert!((edge_brightness(None, 0) - 1.0).abs() < 1e-6);
        assert!((edge_brightness(None, 5) - 0.5).abs() < 1e-6); // mention
        assert!((edge_brightness(None, 6) - 0.55).abs() < 1e-6); // similarity
        assert!((edge_brightness(Some(0.2), 7) - 1.0).abs() < 1e-6); // surprising ignores conf
    }

    #[test]
    fn build_galaxy_flags_surprising_and_bridge() {
        use crate::memory_graph::adapter::{GraphQueryResponse, NoteLinkDto, NoteNodeDto};
        let node = |id: &str| NoteNodeDto {
            id: id.into(),
            name: id.into(),
            path: id.into(),
            category: "c".into(),
            tags: vec![],
            link_count: 1,
            community_id: None,
            updated_at: None,
        };
        let resp = GraphQueryResponse {
            nodes: vec![node("a/x"), node("a/y")],
            edges: vec![NoteLinkDto {
                from: "a/x".into(),
                to: "a/y".into(),
                label: None,
                kind: Some("wikilink".into()),
                confidence: Some(1.0),
            }],
            total: None,
            bridge_nodes: vec!["a/x".into()],
            surprising_edges: vec![("a/x".into(), "a/y".into())],
        };
        let data = build_galaxy(&resp);
        assert_eq!(data.edge_kinds, vec![7], "surprising overrides kind");
        assert_eq!(data.edge_bright.len(), data.edges.len());
        // Bridge node brighter than its sibling.
        assert!(data.nodes[0].color[0] > data.nodes[1].color[0]);
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

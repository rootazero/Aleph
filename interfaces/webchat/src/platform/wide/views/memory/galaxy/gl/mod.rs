//! Pure-Rust WebGL2 renderer for the 3D knowledge nebula.
//!
//! Pure-logic submodules (`math`, `camera`, `layout3d`, `picking`, `fit`,
//! `drift`) are `web-sys`-free and unit-tested on the native target. GL-bound
//! submodules are verified by wasm compile + browser.
pub mod bloom;
pub mod camera;
pub mod context;
pub mod drift;
pub mod edge_lod;
pub mod edges;
pub mod fit;
pub mod layout3d;
pub mod math;
pub mod nodes;
pub mod picking;
pub mod scene;
pub mod shaders;

use math::Vec3;

/// One renderable node in the galaxy. `pos` is mutated by the force layout;
/// everything else is derived once from the RPC DTO.
#[derive(Debug, Clone)]
pub struct GalaxyNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub link_count: u32,
    pub pos: Vec3,
    /// Base RGB in [0,1] (category color, pre-HDR-boost, recency-scaled).
    pub color: [f32; 3],
    /// Louvain community id (`None` on a cold graph cache). Drives spatial
    /// clustering (community centroid gravity in `ForceLayout`).
    pub community: Option<u32>,
}

/// The whole-graph render input. `edges` index into `nodes` (resolved from ids).
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GalaxyNode>,
    pub edges: Vec<(u32, u32)>,
    /// Per-edge relation kind code (see `edges::edge_kind_code`), same order &
    /// length as `edges`. Empty when unknown (treated as all-wikilink).
    pub edge_kinds: Vec<u8>,
    /// Per-edge brightness scale (see `galaxy_build::edge_brightness`), same
    /// order & length as `edges`. Multiplied into the edge color; empty is
    /// treated as full brightness (1.0) everywhere.
    pub edge_bright: Vec<f32>,
}

/// Edges incident to the selected node, as normalized (min,max) index pairs.
/// Drives edge highlight (flow) + non-neighbor dimming.
pub fn compute_highlight_edges(
    data: &GraphData,
    selected_id: &str,
) -> std::collections::HashSet<(u32, u32)> {
    let mut out = std::collections::HashSet::new();
    let Some(sel) = data.nodes.iter().position(|n| n.id == selected_id) else {
        return out;
    };
    let sel = sel as u32;
    for &(a, b) in &data.edges {
        if a == sel || b == sel {
            out.insert((a.min(b), a.max(b)));
        }
    }
    out
}

#[cfg(test)]
mod highlight_tests {
    use super::*;
    use crate::views::memory::galaxy::gl::math::Vec3;

    fn node(id: &str) -> GalaxyNode {
        GalaxyNode {
            id: id.into(),
            name: id.into(),
            category: "x".into(),
            link_count: 0,
            pos: Vec3::zero(),
            color: [1.0, 1.0, 1.0],
            community: None,
        }
    }

    #[test]
    fn highlight_edges_are_neighbor_links_normalized() {
        let data = GraphData {
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![(0, 1), (2, 0), (2, 3)], // a-b, c-a, c-d
            edge_kinds: vec![0; 3],
            edge_bright: vec![1.0; 3],
        };
        let hl = compute_highlight_edges(&data, "a");
        assert!(hl.contains(&(0, 1))); // a-b
        assert!(hl.contains(&(0, 2))); // c-a normalized → (0,2)
        assert!(!hl.contains(&(2, 3))); // c-d not incident to a
    }

    #[test]
    fn unknown_id_yields_empty() {
        let data = GraphData {
            nodes: vec![node("a")],
            edges: vec![],
            edge_kinds: vec![],
            edge_bright: vec![],
        };
        assert!(compute_highlight_edges(&data, "zzz").is_empty());
    }
}

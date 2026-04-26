use super::types::*;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,
    pub backlinks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    pub match_field: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}

/// Deserialization target for the `graph.neighbors` RPC response (radial navigation).
/// Mirrors the server's `GraphNeighborsResponse` in `src/gateway/handlers/graph_types.rs`.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphNeighborsResponse {
    /// The queried node, pinned at world origin in the radial layout.
    pub center: NoteNodeDto,
    /// Neighbor nodes (center excluded).
    pub nodes: Vec<NoteNodeDto>,
    /// Edges between all returned nodes (including center).
    pub edges: Vec<NoteLinkDto>,
    /// Hop distance from center: 1 = direct neighbor, 2 = two hops.
    #[serde(default)]
    pub hop_depth: HashMap<String, u8>,
}

use crate::canvas_engine::cluster::group_by_category_into_clusters;
use crate::canvas_engine::layout::compute_target_positions;

/// Build a `Neighborhood` from a `GraphNeighborsResponse`.
///
/// Index convention (matches layout.rs expectations):
///   0 = center, 1..=one_hop.len() = one_hop nodes, then two_hop nodes.
pub fn to_neighborhood(
    resp: &GraphNeighborsResponse,
    fetched_at_ms: f64,
    fold_threshold: usize,
) -> Neighborhood {
    let center = note_dto_to_canvas(&resp.center, 0);

    let mut one_hop: Vec<CanvasNode> = Vec::new();
    let mut two_hop: Vec<CanvasNode> = Vec::new();
    for n in &resp.nodes {
        let hop = resp.hop_depth.get(&n.id).copied().unwrap_or(2);
        let cn = note_dto_to_canvas(n, hop);
        match hop {
            1 => one_hop.push(cn),
            _ => two_hop.push(cn),
        }
    }

    // Build edges. NoteLinkDto has only `from`/`to` with no relation or weight.
    // All edges from/to the center node are marked as active links.
    let edges: Vec<CanvasEdge> = resp
        .edges
        .iter()
        .map(|e| {
            let from_idx = resolve_idx(&e.from, &resp.center.id, &one_hop, &two_hop);
            let to_idx = resolve_idx(&e.to, &resp.center.id, &one_hop, &two_hop);
            let is_active_link = e.from == resp.center.id || e.to == resp.center.id;
            CanvasEdge {
                from_idx,
                to_idx,
                relation: String::new(),
                is_wikilink: true,
                is_active_link,
            }
        })
        .collect();

    // Top-K folding: show at most `fold_threshold` 1-hop nodes (the highest-weight
    // ones), and fold the remainder into one ClusterNode per category. The previous
    // by-relation grouping was a no-op since NoteLinkDto carries no relation field.
    let (filtered_one_hop, clusters): (Vec<CanvasNode>, Vec<ClusterNode>) =
        if one_hop.len() <= fold_threshold {
            (one_hop, Vec::new())
        } else {
            let mut sorted = one_hop;
            sorted.sort_by(|a, b| {
                let wa = a.decay_score * a.edge_count as f32;
                let wb = b.decay_score * b.edge_count as f32;
                wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
            });
            let kept: Vec<CanvasNode> = sorted.drain(..fold_threshold).collect();
            let folded = group_by_category_into_clusters(sorted, &resp.center.id);
            (kept, folded)
        };

    let target_positions =
        compute_target_positions(&center, &filtered_one_hop, &two_hop, &clusters, &edges);

    // Seed each node's position from its target so the renderer has something
    // to draw immediately. Without a running force simulation the positions
    // would otherwise stay at (0,0) and every node would be stacked on the center.
    let mut center = center;
    if let Some(t) = target_positions.get(&center.id) {
        center.position = Vec2::new(t.x as f64, t.y as f64);
    }
    let mut filtered_one_hop = filtered_one_hop;
    for n in filtered_one_hop.iter_mut() {
        if let Some(t) = target_positions.get(&n.id) {
            n.position = Vec2::new(t.x as f64, t.y as f64);
        }
    }
    let mut two_hop = two_hop;
    for n in two_hop.iter_mut() {
        if let Some(t) = target_positions.get(&n.id) {
            n.position = Vec2::new(t.x as f64, t.y as f64);
        }
    }
    let mut clusters = clusters;
    for c in clusters.iter_mut() {
        if let Some(t) = target_positions.get(&c.id) {
            c.world_pos = Vec2::new(t.x as f64, t.y as f64);
        }
    }

    Neighborhood {
        center,
        one_hop: filtered_one_hop,
        two_hop,
        orphans: Vec::new(),
        clusters,
        edges,
        target_positions,
        fetched_at_ms,
    }
}

/// Populate `nbhd.orphans` with all nodes from `all_dtos` that are not already
/// present in the neighborhood (center, one_hop, two_hop, or cluster members).
///
/// Orphans are laid out evenly around an outer ring at `ORPHAN_RADIUS`, tagged
/// with `hop = ORPHAN_HOP_SENTINEL` and `z = ORPHAN_Z`. They are pinned so the
/// force layout doesn't disturb them, and their world positions are also written
/// into `target_positions` for tween consistency.
pub fn populate_orphans(nbhd: &mut Neighborhood, all_dtos: &[NoteNodeDto]) {
    use std::collections::HashSet;
    use std::f64::consts::TAU;

    let mut in_view: HashSet<&str> = HashSet::new();
    in_view.insert(nbhd.center.id.as_str());
    for n in &nbhd.one_hop {
        in_view.insert(n.id.as_str());
    }
    for n in &nbhd.two_hop {
        in_view.insert(n.id.as_str());
    }
    for c in &nbhd.clusters {
        for id in &c.member_ids {
            in_view.insert(id.as_str());
        }
    }

    let orphan_dtos: Vec<&NoteNodeDto> = all_dtos
        .iter()
        .filter(|d| !in_view.contains(d.id.as_str()))
        .collect();

    let count = orphan_dtos.len();
    if count == 0 {
        nbhd.orphans = Vec::new();
        return;
    }

    let r = ORPHAN_RADIUS as f64;
    // Stagger by a quarter turn so the first orphan doesn't sit on the +X axis.
    let phase = TAU / 8.0;

    let mut orphans = Vec::with_capacity(count);
    for (i, dto) in orphan_dtos.into_iter().enumerate() {
        let angle = phase + (i as f64) * TAU / (count as f64);
        let x = angle.cos() * r;
        let y = angle.sin() * r;

        let mut node = note_dto_to_canvas(dto, ORPHAN_HOP_SENTINEL);
        node.position = Vec2::new(x, y);
        node.z = ORPHAN_Z;
        node.pinned = true;
        // Shrink ghost dots: orphans are visual context, not focus targets.
        node.radius = 4.5;
        orphans.push(node);

        nbhd.target_positions
            .insert(dto.id.clone(), Vec3::new(x as f32, y as f32, ORPHAN_Z));
    }
    nbhd.orphans = orphans;
}

fn note_dto_to_canvas(dto: &NoteNodeDto, hop: u8) -> CanvasNode {
    let z = match hop {
        0 => 0.0_f32,
        1 => 60.0,
        _ => 140.0,
    };
    CanvasNode {
        id: dto.id.clone(),
        name: dto.name.clone(),
        category: dto.category.clone(),
        color: NOTE_COLOR,
        radius: note_radius(dto.link_count),
        position: Vec2::zero(),
        velocity: Vec2::zero(),
        pinned: hop == 0,
        z,
        hop,
        decay_score: 1.0,
        edge_count: dto.link_count,
    }
}

fn resolve_idx(
    id: &str,
    center_id: &str,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
) -> usize {
    if id == center_id {
        return 0;
    }
    if let Some(p) = one_hop.iter().position(|n| n.id == id) {
        return p + 1;
    }
    if let Some(p) = two_hop.iter().position(|n| n.id == id) {
        return one_hop.len() + 1 + p;
    }
    0 // fallback to center; edge referencing an unknown node
}

pub fn adapt_graph_response(response: &GraphQueryResponse) -> (Vec<CanvasNode>, Vec<CanvasEdge>) {
    let total = response.nodes.len();
    let nodes: Vec<CanvasNode> = response
        .nodes
        .iter()
        .enumerate()
        .map(|(i, dto)| {
            let angle = (i as f64 / total.max(1) as f64) * std::f64::consts::TAU;
            let spread = 200.0;
            CanvasNode {
                id: dto.id.clone(),
                name: dto.name.clone(),
                category: dto.category.clone(),
                color: NOTE_COLOR,
                radius: note_radius(dto.link_count),
                position: Vec2::new(angle.cos() * spread, angle.sin() * spread),
                velocity: Vec2::zero(),
                pinned: false,
                z: 0.0,
                hop: 2, // placeholder; overwritten by to_neighborhood (Task 11)
                decay_score: 1.0, // no decay data in global-graph response; populated in radial mode
                edge_count: dto.link_count,
            }
        })
        .collect();

    let id_to_idx: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let edges: Vec<CanvasEdge> = response
        .edges
        .iter()
        .filter_map(|dto| {
            let from_idx = id_to_idx.get(dto.from.as_str()).copied()?;
            let to_idx = id_to_idx.get(dto.to.as_str()).copied()?;
            Some(CanvasEdge {
                from_idx,
                to_idx,
                relation: String::new(),
                is_wikilink: true,
                is_active_link: false,
            })
        })
        .collect();

    (nodes, edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(id: &str, category: &str) -> NoteNodeDto {
        NoteNodeDto {
            id: id.to_string(),
            name: id.to_string(),
            path: format!("{id}.md"),
            category: category.to_string(),
            tags: vec![],
            link_count: 1,
        }
    }

    fn make_resp_with_n_one_hop(n: usize, category: &str) -> GraphNeighborsResponse {
        let mut resp = GraphNeighborsResponse {
            center: NoteNodeDto {
                id: "center".to_string(),
                name: "center".to_string(),
                path: "center.md".to_string(),
                category: "concept".to_string(),
                tags: vec![],
                link_count: 1,
            },
            nodes: Vec::new(),
            edges: Vec::new(),
            hop_depth: HashMap::new(),
        };
        for i in 0..n {
            let id = format!("n{i}");
            resp.nodes.push(NoteNodeDto {
                id: id.clone(),
                name: id.clone(),
                path: format!("n{i}.md"),
                category: category.to_string(),
                tags: vec![],
                link_count: 1,
            });
            resp.hop_depth.insert(id, 1);
        }
        resp
    }

    #[test]
    fn top_k_fold_keeps_all_when_under_threshold() {
        let resp = make_resp_with_n_one_hop(8, "concept");
        let nb = to_neighborhood(&resp, 0.0, 12);
        assert_eq!(nb.one_hop.len(), 8);
        assert_eq!(nb.clusters.len(), 0);
    }

    #[test]
    fn top_k_fold_keeps_top_k_by_weight() {
        let mut resp = make_resp_with_n_one_hop(20, "concept");
        // Set weights: node "n0" highest, "n19" lowest
        for (i, n) in resp.nodes.iter_mut().enumerate() {
            n.link_count = (20 - i).max(1);
        }
        let nb = to_neighborhood(&resp, 0.0, 5);
        assert_eq!(nb.one_hop.len(), 5);
        let kept_ids: Vec<&str> = nb.one_hop.iter().map(|n| n.id.as_str()).collect();
        for i in 0..5 {
            let expected = format!("n{i}");
            assert!(
                kept_ids.contains(&expected.as_str()),
                "n{i} should be kept (top weight)"
            );
        }
    }

    #[test]
    fn top_k_fold_remainder_splits_by_category() {
        // 3 categories of 8 each = 24 total; threshold=10 => 10 unfolded, 14 in clusters across categories.
        // Weights are interleaved across categories (round-robin) so the top-10 by weight
        // always spans all 3 categories, guaranteeing each category appears in the remainder.
        let cats = ["concept", "reference", "topic"];
        let mut resp = make_resp_with_n_one_hop(0, "concept");
        // Insert 24 nodes interleaved: concept/reference/topic/concept/...
        // link_count descends from 24 so node 0 is heaviest.
        for global_idx in 0..24usize {
            let cat = cats[global_idx % 3];
            let id = format!("n{global_idx}");
            resp.nodes.push(NoteNodeDto {
                id: id.clone(),
                name: format!("name{global_idx}"),
                path: format!("n{global_idx}.md"),
                category: cat.to_string(),
                tags: vec![],
                link_count: 24 - global_idx, // unique, descending weight
            });
            resp.hop_depth.insert(id, 1);
        }
        // Top 10 by weight = indices 0-9 → 4 concept (0,3,6,9) + 3 reference (1,4,7) + 3 topic (2,5,8).
        // Remainder 14 = 4 concept + 5 reference + 5 topic → 3 clusters.
        let nb = to_neighborhood(&resp, 0.0, 10);
        assert_eq!(nb.one_hop.len(), 10);
        assert_eq!(nb.clusters.len(), 3, "one cluster per category");
        let total_in_clusters: usize = nb.clusters.iter().map(|c| c.member_ids.len()).sum();
        assert_eq!(total_in_clusters, 14);
    }

    #[test]
    fn to_neighborhood_basic_shape() {
        let resp = GraphNeighborsResponse {
            center: dto("a", "concept"),
            nodes: vec![dto("b", "person"), dto("c", "tool")],
            edges: vec![
                NoteLinkDto { from: "a".to_string(), to: "b".to_string() },
                NoteLinkDto { from: "a".to_string(), to: "c".to_string() },
            ],
            hop_depth: [("b".to_string(), 1u8), ("c".to_string(), 2u8)]
                .iter()
                .cloned()
                .collect(),
        };
        let nb = to_neighborhood(&resp, 0.0, 12);
        assert_eq!(nb.center.id, "a");
        assert_eq!(nb.center.hop, 0);
        // "b" is hop=1, "c" is hop=2
        // Neither group hits threshold=12, so no clusters.
        assert_eq!(nb.clusters.len(), 0);
        let one_hop_total = nb.one_hop.len()
            + nb.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
        assert_eq!(one_hop_total, 1, "only 'b' is 1-hop");
        assert_eq!(nb.two_hop.len(), 1, "only 'c' is 2-hop");
        assert_eq!(nb.two_hop[0].id, "c");
        // Center should be pinned (hop=0)
        assert!(nb.center.pinned);
        // target_positions must have entries for all nodes
        assert!(nb.target_positions.contains_key("a"));
        assert!(nb.target_positions.contains_key("b"));
        assert!(nb.target_positions.contains_key("c"));
    }
}

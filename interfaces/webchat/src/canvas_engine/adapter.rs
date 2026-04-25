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

use crate::canvas_engine::cluster::{fallback_fold, fold_sector};
use crate::canvas_engine::layout::compute_target_positions;

/// Build a `Neighborhood` from a `GraphNeighborsResponse`.
///
/// Index convention (matches layout.rs expectations):
///   0 = center, 1..=one_hop.len() = one_hop nodes, then two_hop nodes.
pub fn to_neighborhood(resp: &GraphNeighborsResponse, fetched_at_ms: f64) -> Neighborhood {
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

    // Group 1-hop nodes by the relation label on the edge to center.
    // Since NoteLinkDto carries no relation field, we use "_default" for all.
    // The group key is still useful as a stable sector for layout.
    let mut by_relation: HashMap<String, Vec<CanvasNode>> = HashMap::new();
    for (i, n) in one_hop.iter().enumerate() {
        let neighbor_idx = i + 1;
        let rel = edges
            .iter()
            .find(|e| {
                e.is_active_link
                    && ((e.from_idx == 0 && e.to_idx == neighbor_idx)
                        || (e.to_idx == 0 && e.from_idx == neighbor_idx))
            })
            .map(|e| e.relation.clone())
            .unwrap_or_else(|| "_default".to_string());
        by_relation.entry(rel).or_default().push(n.clone());
    }

    let mut filtered_one_hop: Vec<CanvasNode> = Vec::new();
    let mut clusters: Vec<ClusterNode> = Vec::new();
    for (rel, group) in by_relation {
        let (mut unfolded, mut group_clusters) =
            fold_sector(&group, &rel, &resp.center.id, 12);
        if unfolded.len() >= 30 {
            let (kept, more_clusters) = fallback_fold(unfolded, &rel, &resp.center.id);
            unfolded = kept;
            group_clusters.extend(more_clusters);
        }
        filtered_one_hop.extend(unfolded);
        clusters.extend(group_clusters);
    }

    let target_positions =
        compute_target_positions(&center, &filtered_one_hop, &two_hop, &clusters, &edges);

    Neighborhood {
        center,
        one_hop: filtered_one_hop,
        two_hop,
        clusters,
        edges,
        target_positions,
        fetched_at_ms,
    }
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
        let nb = to_neighborhood(&resp, 0.0);
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

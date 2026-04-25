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
                hop: 2,
                decay_score: 1.0,
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

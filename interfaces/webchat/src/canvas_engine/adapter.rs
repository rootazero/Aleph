use serde::Deserialize;
use std::collections::HashMap;
use super::types::*;

#[derive(Debug, Clone, Deserialize)]
pub struct GraphNodeDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub decay_score: f32,
    pub edge_count: usize,
    pub has_wiki: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphEdgeDto {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WikiDto {
    pub id: String,
    pub content: String,
    pub fact_source: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactDto {
    pub id: String,
    pub content: String,
    pub confidence: f32,
    pub fact_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDetailResponse {
    pub node: GraphNodeDto,
    pub wiki: Option<WikiDto>,
    pub facts: Vec<FactDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub kind: String,
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
            let weight = dto.decay_score as f64 * (dto.edge_count as f64 + 1.0).ln();
            let radius = 10.0 + (weight * 4.0).min(20.0);
            let angle = (i as f64 / total.max(1) as f64) * std::f64::consts::TAU;
            let spread = 200.0;
            CanvasNode {
                id: dto.id.clone(),
                name: dto.name.clone(),
                kind: dto.kind.clone(),
                aliases: dto.aliases.clone(),
                icon: kind_icon(&dto.kind),
                color: kind_color(&dto.kind),
                radius,
                has_wiki: dto.has_wiki,
                position: Vec2::new(angle.cos() * spread, angle.sin() * spread),
                velocity: Vec2::zero(),
                pinned: false,
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
            let from_idx = id_to_idx.get(dto.from_id.as_str()).copied()?;
            let to_idx = id_to_idx.get(dto.to_id.as_str()).copied()?;
            Some(CanvasEdge {
                from_idx,
                to_idx,
                relation: dto.relation.clone(),
                is_wikilink: dto.relation == "references",
            })
        })
        .collect();

    (nodes, edges)
}

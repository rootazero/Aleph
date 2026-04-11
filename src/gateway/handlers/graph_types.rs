use serde::{Deserialize, Serialize};

// === graph.query ===
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub kind_filter: Vec<String>,
}
fn default_limit() -> usize {
    100
}

#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Serialize)]
pub struct GraphNodeDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub decay_score: f32,
    pub edge_count: usize,
    pub has_wiki: bool,
}

#[derive(Debug, Serialize)]
pub struct GraphEdgeDto {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
}

// === graph.neighbors ===
#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}
fn default_depth() -> u8 {
    2
}
fn default_neighbor_limit() -> usize {
    50
}

// === graph.node_detail ===
#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
}

#[derive(Debug, Serialize)]
pub struct GraphNodeDetailResponse {
    pub node: GraphNodeDto,
    pub wiki: Option<WikiDto>,
    pub facts: Vec<FactDto>,
}

#[derive(Debug, Serialize)]
pub struct WikiDto {
    pub id: String,
    pub content: String,
    pub fact_source: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct FactDto {
    pub id: String,
    pub content: String,
    pub confidence: f32,
    pub fact_type: String,
}

// === graph.search ===
#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}
fn default_search_limit() -> usize {
    10
}

#[derive(Debug, Serialize)]
pub struct GraphSearchResponse {
    pub results: Vec<GraphSearchResult>,
}

#[derive(Debug, Serialize)]
pub struct GraphSearchResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub match_field: String,
}

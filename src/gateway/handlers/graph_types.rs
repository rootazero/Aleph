use serde::{Deserialize, Serialize};

// === graph.query ===
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
fn default_limit() -> usize {
    100
}

// === graph.neighbors ===
#[derive(Debug, Deserialize)]
pub struct GraphNeighborsParams {
    pub node_id: String,
    #[serde(default = "default_depth")]
    pub depth: u8,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
fn default_depth() -> u8 {
    2
}
fn default_neighbor_limit() -> usize {
    200
}

// === graph.node_detail ===
#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// === graph.search ===
#[derive(Debug, Deserialize)]
pub struct GraphSearchParams {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
fn default_search_limit() -> usize {
    20
}

// === Response types ===

#[derive(Debug, Serialize, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,   // path: "wiki/rust-ownership"
    pub name: String, // display: "rust-ownership" (filename only)
    pub path: String, // full relative path
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
}

#[derive(Debug, Serialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,
    pub backlinks: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    pub match_field: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}

// === graph.neighbors response (radial navigation) ===
#[derive(Debug, Serialize, Deserialize)]
pub struct GraphNeighborsResponse {
    /// The node that was queried — pinned at world origin in the radial layout.
    pub center: NoteNodeDto,
    /// Neighbor nodes (excludes the center node itself).
    pub nodes: Vec<NoteNodeDto>,
    /// Edges between all returned nodes (including center).
    pub edges: Vec<NoteLinkDto>,
    /// Hop distance from center for each neighbor node: 1 = direct, 2 = two hops.
    pub hop_depth: std::collections::HashMap<String, u8>,
}

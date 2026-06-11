use serde::{Deserialize, Serialize};

// === graph.query ===
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
const fn default_limit() -> usize {
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
const fn default_depth() -> u8 {
    2
}
const fn default_neighbor_limit() -> usize {
    200
}

// === graph.node_detail ===
#[derive(Debug, Deserialize)]
pub struct GraphNodeDetailParams {
    pub node_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// === graph.update_note ===
#[derive(Debug, Deserialize)]
pub struct GraphUpdateNoteParams {
    /// Note path `"category/title"` (same id used by `graph.node_detail`).
    pub node_id: String,
    /// Full raw markdown (frontmatter + body) to persist verbatim.
    pub content: String,
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
const fn default_search_limit() -> usize {
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
    /// Free-form display label for the edge, e.g. the wikilink alias from
    /// `[[target|alias]]`. Matches the Obsidian JSON Canvas `edge.label` slot.
    /// `None` until the storage layer learns to surface aliases (tracked in
    /// the R2 follow-up "wikilink-kind-extraction").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Semantic relation kind, e.g. `"refers"` / `"derives"` / `"follows"` /
    /// `"related"`. Directional kinds export as arrow-headed edges in the
    /// panel's JSON Canvas export; the on-canvas renderer draws all edges as
    /// plain strokes. `None` until the writer pipeline extracts kind hints
    /// from note bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
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

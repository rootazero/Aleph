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

// === graph.rename_note ===
#[derive(Debug, Deserialize)]
pub struct GraphRenameNoteParams {
    /// Note path `"category/title"` (same id as `graph.node_detail`).
    pub node_id: String,
    /// New filename (without `.md`); sanitized server-side.
    pub new_title: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// === graph.delete_note ===
#[derive(Debug, Deserialize)]
pub struct GraphDeleteNoteParams {
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
    /// Community id from `notes_graph_cache` (Louvain). `None` on a cold cache
    /// (before the first dream graph-recompute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub community_id: Option<u32>,
    /// Note last-modified epoch seconds — drives recency-based visual encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
    /// Free-form display label for the edge, e.g. the wikilink alias from
    /// `[[target|alias]]`. Matches the Obsidian JSON Canvas `edge.label` slot.
    /// `Some(alias)` when the link was written with a pipe alias; `None` for
    /// plain `[[target]]` wikilinks and typed relations (which carry no
    /// alias text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Semantic relation kind, e.g. `"refers"` / `"derives"` / `"follows"` /
    /// `"related"`. Directional kinds export as arrow-headed edges in the
    /// panel's JSON Canvas export; the on-canvas renderer draws all edges as
    /// plain strokes. `None` until the writer pipeline extracts kind hints
    /// from note bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Link-resolution confidence (0.0-1.0) for real edges; `None` for
    /// similarity edges (`kind = "related_similarity"`), whose relatedness
    /// score is not a confidence and must not be presented as one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    /// Total notes for the agent (nodes may be truncated to `limit`); lets the
    /// panel show a "showing top N of M" indicator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    /// Node paths materialized as graph-health "bridge" insights (cut
    /// vertices connecting otherwise-separate clusters), filtered to nodes
    /// visible in this response. Empty before the first dream graph-recompute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bridge_nodes: Vec<String>,
    /// `(from, to)` pairs materialized as graph-health "surprising" insights
    /// (unexpectedly strong cross-community edges), filtered to pairs whose
    /// endpoints are both visible in this response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surprising_edges: Vec<(String, String)>,
}

/// One outgoing link row for `graph.node_detail`, carrying full lifecycle
/// provenance (unlike the graph feed's active-only edges) so the panel can
/// render dangling/tombstone links distinctly.
#[derive(Debug, Serialize)]
pub struct OutgoingLinkDto {
    pub to: String, // resolved path (active/tombstone) or raw (dangling)
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    pub status: String, // active | dangling | tombstone
}

#[derive(Debug, Serialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,
    pub backlinks: Vec<String>,
    pub outgoing: Vec<OutgoingLinkDto>,
}

/// One full-text search hit.
///
/// Carries the whole index row, not just an id: the panel renders hits as note
/// cards, and a hit that only knows its own name would force a second round
/// trip per row. Every field below is already on `NoteIndexEntry`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    /// `"title"` when the query matched the filename, `"content"` otherwise.
    pub match_field: String,
    pub agent_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub tags: Vec<String>,
    pub link_count: usize,
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

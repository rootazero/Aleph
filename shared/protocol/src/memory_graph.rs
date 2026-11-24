//! Read contracts for the Memory Nebula graph, search, and note detail.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,   // path: "wiki/rust-ownership"
    pub name: String, // display: "rust-ownership" (filename only)
    pub path: String, // full relative path
    /// Exact partition read by Core. Older servers omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// `"related"`. The endpoints are directed independently of how a panel
    /// draws the row. `None` until the writer extracts kind hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Link-resolution confidence (0.0-1.0) for real edges; `None` for
    /// similarity edges (`kind = "related_similarity"`), whose relatedness
    /// score is not a confidence and must not be presented as one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}

/// An exact note address, distinct from an agent selector for enumeration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteAddress {
    pub agent_id: String,
    pub node_id: String,
}

impl NoteNodeDto {
    pub fn address(&self) -> Option<NoteAddress> {
        Some(NoteAddress {
            agent_id: self.agent_id.clone()?,
            node_id: self.id.clone(),
        })
    }
}

//! Deserialization targets (DTOs) for the `graph.*` JSON-RPC responses consumed
//! by the memory canvas. Pure wire structs — no layout/adaptation logic (the 2D
//! radial engine that once lived here was retired with the 3D galaxy).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link_count: usize,
    /// Louvain community id (`None` on a cold graph cache). Drives community
    /// clustering/coloring in the galaxy (consumed by Plan 3).
    #[serde(default)]
    pub community_id: Option<u32>,
    /// Note last-modified epoch seconds — drives recency visual encoding.
    #[serde(default)]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    /// Total notes for the agent; `nodes` may be truncated to the query limit.
    #[serde(default)]
    pub total: Option<usize>,
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

pub use aleph_protocol::memory_graph::{
    GraphQueryResponse, GraphSearchResponse, NoteAddress, NoteDetailResponse, NoteLinkDto,
    NoteNodeDto, OutgoingLinkDto, SearchResultDto,
};
use serde::Deserialize;

// === graph.query ===
#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    #[serde(default)]
    pub focus: Option<NoteAddress>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub agent_id: Option<String>,
}
const fn default_limit() -> usize {
    100
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

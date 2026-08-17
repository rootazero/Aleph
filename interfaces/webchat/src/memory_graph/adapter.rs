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
    /// Link-resolution confidence (0.0-1.0) for real edges; `None` for
    /// similarity edges (`kind = "related_similarity"`). Drives edge
    /// brightness in the galaxy renderer (see `galaxy_build::edge_brightness`).
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    /// Total notes for the agent; `nodes` may be truncated to the query limit.
    #[serde(default)]
    pub total: Option<usize>,
    /// Node paths materialized as graph-health "bridge" insights, filtered to
    /// nodes visible in this response. Empty before the first dream recompute.
    #[serde(default)]
    pub bridge_nodes: Vec<String>,
    /// `(from, to)` pairs materialized as graph-health "surprising" insights,
    /// filtered to pairs whose endpoints are both visible in this response.
    #[serde(default)]
    pub surprising_edges: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutgoingLinkDto {
    pub to: String,
    pub raw: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    pub confidence: f32,
    #[serde(default)]
    pub resolved_by: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteDetailResponse {
    pub node: NoteNodeDto,
    pub content: String,
    pub backlinks: Vec<String>,
    #[serde(default)]
    pub outgoing: Vec<OutgoingLinkDto>,
}

/// One `graph.search` hit — a full note index row, mirroring the server DTO.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    pub match_field: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub link_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphSearchResponse {
    pub results: Vec<SearchResultDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_link_dto_deserializes_with_defaults() {
        let json = r#"{
            "to": "wiki/rust",
            "raw": "Rust",
            "confidence": 0.95,
            "status": "active"
        }"#;
        let link: OutgoingLinkDto = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(link.to, "wiki/rust");
        assert_eq!(link.raw, "Rust");
        assert_eq!(link.confidence, 0.95);
        assert_eq!(link.status, "active");
        assert_eq!(link.relation, None);
        assert_eq!(link.label, None);
        assert_eq!(link.resolved_by, None);
    }

    /// A narrow response from an un-upgraded core (Panel connected to an older
    /// gateway over LAN, per the "Panel ↔ Daemon" deployment split) must still
    /// parse — the new fields default rather than fail the whole response.
    #[test]
    fn search_result_dto_deserializes_without_the_new_fields() {
        let json = r#"{
            "id": "wiki/rust",
            "name": "rust",
            "category": "wiki",
            "match_field": "content"
        }"#;
        let hit: SearchResultDto = serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(hit.id, "wiki/rust");
        assert_eq!(hit.agent_id, "");
        assert_eq!(hit.created_at, 0);
        assert_eq!(hit.updated_at, 0);
        assert!(hit.tags.is_empty());
        assert_eq!(hit.link_count, 0);
    }

    #[test]
    fn note_detail_response_deserializes_without_outgoing() {
        let json = r##"{
            "node": {
                "id": "wiki/test",
                "name": "test",
                "path": "wiki/test.md",
                "category": "wiki",
                "link_count": 5
            },
            "content": "# Test",
            "backlinks": ["wiki/other"]
        }"##;
        let response: NoteDetailResponse =
            serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(response.node.id, "wiki/test");
        assert_eq!(response.content, "# Test");
        assert_eq!(response.backlinks.len(), 1);
        assert!(response.outgoing.is_empty());
    }

    #[test]
    fn note_detail_response_deserializes_with_outgoing() {
        let json = r##"{
            "node": {
                "id": "wiki/test",
                "name": "test",
                "path": "wiki/test.md",
                "category": "wiki",
                "link_count": 5
            },
            "content": "# Test",
            "backlinks": ["wiki/other"],
            "outgoing": [
                {
                    "to": "wiki/rust",
                    "raw": "Rust",
                    "relation": "refers",
                    "label": "Ownership",
                    "confidence": 0.95,
                    "status": "active"
                }
            ]
        }"##;
        let response: NoteDetailResponse =
            serde_json::from_str(json).expect("Failed to deserialize");
        assert_eq!(response.outgoing.len(), 1);
        assert_eq!(response.outgoing[0].to, "wiki/rust");
        assert_eq!(response.outgoing[0].relation, Some("refers".to_string()));
    }
}

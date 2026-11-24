//! Legacy imports for the shared memory graph read contracts.

pub use aleph_protocol::memory_graph::{
    GraphQueryResponse, GraphSearchResponse, NoteDetailResponse, OutgoingLinkDto, SearchResultDto,
};
#[cfg(test)]
pub use aleph_protocol::memory_graph::{NoteLinkDto, NoteNodeDto};

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

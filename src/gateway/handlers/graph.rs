//! Graph Query Handler
//!
//! Handles JSON-RPC requests for knowledge graph visualization.
//! These handlers query the NoteStore for note index data and links.

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::graph_types::{
    GraphNeighborsParams, GraphNeighborsResponse, GraphNodeDetailParams, GraphQueryParams,
    GraphQueryResponse, GraphSearchParams, GraphSearchResponse, NoteDetailResponse, NoteLinkDto,
    NoteNodeDto, SearchResultDto,
};
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::store::MemoryBackend;

/// Convert a NoteIndexEntry into a NoteNodeDto.
fn entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto {
    NoteNodeDto {
        id: entry.path.clone(),
        name: entry.filename.clone(),
        path: entry.path.clone(),
        category: entry.category.clone(),
        tags: entry.tags.clone(),
        link_count: entry.link_count,
    }
}

/// Resolve the note memory directory: `~/.aleph/memory/note/`
fn notes_dir() -> std::path::PathBuf {
    crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("aleph")
            .join("memory")
            .join("note")
    })
}

/// Handle graph.query — returns nodes and edges for visualization.
///
/// Requires NoteStore wired at Gateway startup.
pub async fn handle_query(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.query requires NoteStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.neighbors — returns neighbors of a node up to a given depth.
///
/// Requires NoteStore wired at Gateway startup.
pub async fn handle_neighbors(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.neighbors requires NoteStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.node_detail — returns full detail for a single note.
///
/// Requires NoteStore wired at Gateway startup.
pub async fn handle_node_detail(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.node_detail requires NoteStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.search — full-text search over notes.
///
/// Requires NoteStore wired at Gateway startup.
pub async fn handle_search(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.search requires NoteStore — wire in Gateway startup".to_string(),
    )
}

// ============================================================================
// Real implementation functions (wired at Gateway startup)
// ============================================================================

/// Real implementation of graph.query.
///
/// Returns notes sorted by link_count + recency (up to `limit`),
/// plus all edges (links) between the returned notes.
pub async fn handle_query_impl(req: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GraphQueryParams = match serde_json::from_value(
        req.params
            .clone()
            .unwrap_or(serde_json::Value::Object(Default::default())),
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INVALID_PARAMS, format!("Invalid params: {e}"))
        }
    };

    let (entries, links) = match db
        .get_graph_data(crate::routing::DEFAULT_AGENT_ID, params.limit)
        .await
    {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    let nodes: Vec<NoteNodeDto> = entries.iter().map(entry_to_dto).collect();
    let edges: Vec<NoteLinkDto> = links
        .into_iter()
        .map(|(from, to)| NoteLinkDto { from, to })
        .collect();

    let response = GraphQueryResponse { nodes, edges };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Real implementation of graph.neighbors.
///
/// BFS from the given `node_id` up to `depth` hops, collecting up to `limit`
/// neighbour notes and all edges between them.
pub async fn handle_neighbors_impl(req: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GraphNeighborsParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required param: node_id".to_string(),
            )
        }
    };

    let (entries, links) = match db
        .get_neighbors(
            &params.node_id,
            crate::routing::DEFAULT_AGENT_ID,
            params.depth,
            params.limit,
        )
        .await
    {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Look up the center node entry so the frontend can pin it at world origin.
    let center_entry = match db
        .get_note_index(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
    {
        Ok(Some(e)) => e,
        Ok(None) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("node not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };
    let center = entry_to_dto(&center_entry);

    // Exclude the center node itself from the neighbor list — it is already in `center`.
    let nodes: Vec<NoteNodeDto> = entries
        .iter()
        .filter(|e| e.path != params.node_id)
        .map(entry_to_dto)
        .collect();
    let edges: Vec<NoteLinkDto> = links
        .into_iter()
        .map(|(from, to)| NoteLinkDto { from, to })
        .collect();

    // Compute hop distance (1 = direct edge to/from center, 2 = further).
    let mut hop_depth = std::collections::HashMap::new();
    for n in &nodes {
        let depth = compute_hop_depth(&params.node_id, &n.id, &edges);
        hop_depth.insert(n.id.clone(), depth);
    }

    let response = GraphNeighborsResponse {
        center,
        nodes,
        edges,
        hop_depth,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Real implementation of graph.node_detail.
///
/// Returns the note index entry, full markdown content (read from disk),
/// and backlinks (incoming links from other notes).
pub async fn handle_node_detail_impl(req: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GraphNodeDetailParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required param: node_id".to_string(),
            )
        }
    };

    // Fetch the note index entry.
    let entry = match db
        .get_note_index(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
    {
        Ok(Some(e)) => e,
        Ok(None) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Note not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Read the markdown file from disk using the full path (includes category subdirectory).
    let agent_id = crate::routing::DEFAULT_AGENT_ID; // TODO: derive from request when multi-agent is wired
    let md_path = notes_dir()
        .join(agent_id)
        .join(format!("{}.md", entry.path));
    let content = tokio::fs::read_to_string(&md_path)
        .await
        .unwrap_or_default();

    // Fetch backlinks (incoming links).
    let backlinks = db
        .get_incoming_links(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
        .unwrap_or_default();

    let node = entry_to_dto(&entry);
    let response = NoteDetailResponse {
        node,
        content,
        backlinks,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Compute hop distance from `center_id` to `target_id` given the edge list.
///
/// Returns 0 if they are the same node, 1 if directly connected by an edge,
/// or 2 otherwise (all other nodes in a depth-2 BFS result).
fn compute_hop_depth(center_id: &str, target_id: &str, edges: &[NoteLinkDto]) -> u8 {
    if center_id == target_id {
        return 0;
    }
    let directly_connected = edges.iter().any(|e| {
        (e.from == center_id && e.to == target_id) || (e.to == center_id && e.from == target_id)
    });
    if directly_connected {
        1
    } else {
        2
    }
}

/// Real implementation of graph.search.
///
/// Full-text search over note content via NoteStore FTS index.
pub async fn handle_search_impl(req: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: GraphSearchParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required param: query".to_string(),
            )
        }
    };

    let entries = match db
        .search_notes_fts(
            &params.query,
            crate::routing::DEFAULT_AGENT_ID,
            params.limit,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    let results: Vec<SearchResultDto> = entries
        .into_iter()
        .map(|entry| {
            // Determine match field heuristic: check if filename contains the query.
            let match_field = if entry
                .filename
                .to_lowercase()
                .contains(&params.query.to_lowercase())
            {
                "title".to_string()
            } else {
                "content".to_string()
            };
            SearchResultDto {
                id: entry.path.clone(),
                name: entry.filename,
                category: entry.category,
                match_field,
            }
        })
        .collect();

    let response = GraphSearchResponse { results };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use uuid::Uuid;

    /// Create a fresh file-backed `MemoryBackend` for testing.
    fn make_db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("graph_handler_test_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn make_note(title: &str, category: &str, links: Vec<&str>) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            tags: vec![],
            facts: vec!["fact".to_string()],
            links: links.into_iter().map(String::from).collect(),
            created_at: 1_700_000_000,
            updated_at: 1_700_001_000,
            content_hash: format!("hash_{title}"),
        }
    }

    fn neighbors_request(node_id: &str, depth: u8, limit: usize) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.neighbors".to_string(),
            params: Some(serde_json::json!({
                "node_id": node_id,
                "depth": depth,
                "limit": limit,
            })),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn graph_neighbors_returns_center_and_hop_depth() {
        let db = make_db();
        let agent = crate::routing::DEFAULT_AGENT_ID;

        // Seed: center="concept/Rust", hop-1="concept/Cargo", hop-2="concept/Clippy"
        // Rust → Cargo, Cargo → Clippy. Link targets must be full paths so the
        // BFS in get_neighbors (which matches to_note against from_note) traverses.
        let rust = make_note("Rust", "concept", vec!["concept/Cargo"]);
        let cargo = make_note("Cargo", "concept", vec!["concept/Clippy"]);
        let clippy = make_note("Clippy", "concept", vec![]);
        db.index_note(&rust, agent, "concept").await.unwrap();
        db.index_note(&cargo, agent, "concept").await.unwrap();
        db.index_note(&clippy, agent, "concept").await.unwrap();

        let center_id = "concept/Rust";
        let req = neighbors_request(center_id, 2, 50);
        let resp_raw = handle_neighbors_impl(req, db).await;

        // Must be a success response
        assert!(
            resp_raw.error.is_none(),
            "expected success, got error: {:?}",
            resp_raw.error
        );
        let result = resp_raw.result.expect("result must be present");

        // Deserialize into GraphNeighborsResponse
        let resp: GraphNeighborsResponse = serde_json::from_value(result).expect("deserialize");

        // center field must equal the requested node_id
        assert_eq!(
            resp.center.id, center_id,
            "center.id must equal request node_id"
        );
        assert_eq!(resp.center.name, "Rust");

        // hop_depth must be populated for every returned neighbor
        assert!(!resp.hop_depth.is_empty(), "hop_depth must not be empty");
        for node in &resp.nodes {
            let hop = resp.hop_depth.get(&node.id).copied();
            assert!(
                matches!(hop, Some(1) | Some(2)),
                "hop for {} must be 1 or 2, got {:?}",
                node.id,
                hop
            );
        }

        // concept/Cargo is directly linked from Rust → must be hop 1
        if let Some(&h) = resp.hop_depth.get("concept/Cargo") {
            assert_eq!(h, 1, "concept/Cargo must be hop 1");
        }
    }

    #[tokio::test]
    async fn graph_neighbors_returns_not_found_for_missing_node() {
        let db = make_db();
        let req = neighbors_request("concept/DoesNotExist", 2, 50);
        let resp = handle_neighbors_impl(req, db).await;
        assert!(
            resp.error.is_some(),
            "expected error for missing center node"
        );
    }

    #[test]
    fn compute_hop_depth_direct_edge() {
        let edges = vec![NoteLinkDto {
            from: "A".to_string(),
            to: "B".to_string(),
        }];
        assert_eq!(compute_hop_depth("A", "B", &edges), 1);
        assert_eq!(compute_hop_depth("B", "A", &edges), 1); // reverse edge
        assert_eq!(compute_hop_depth("A", "C", &edges), 2); // not connected
        assert_eq!(compute_hop_depth("A", "A", &edges), 0); // self
    }
}

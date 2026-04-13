//! Graph Query Handler
//!
//! Handles JSON-RPC requests for knowledge graph visualization.
//! These handlers query the NoteStore for note index data and links.

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::graph_types::{
    GraphNeighborsParams, GraphNodeDetailParams, GraphQueryParams, GraphQueryResponse,
    GraphSearchParams, GraphSearchResponse, NoteDetailResponse, NoteLinkDto, NoteNodeDto,
    SearchResultDto,
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
    let content = match tokio::fs::read_to_string(&md_path).await {
        Ok(c) => c,
        Err(_) => String::new(), // graceful fallback if file is missing
    };

    // Fetch backlinks (incoming links).
    let backlinks = match db
        .get_incoming_links(&params.node_id, crate::routing::DEFAULT_AGENT_ID)
        .await
    {
        Ok(links) => links,
        Err(_) => Vec::new(),
    };

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

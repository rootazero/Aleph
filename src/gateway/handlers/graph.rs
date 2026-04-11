//! Graph Query Handler
//!
//! Handles JSON-RPC requests for knowledge graph visualization.
//! These handlers are placeholders — actual GraphStore wiring happens at Gateway startup (Task 18).

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Handle graph.query — returns nodes and edges for visualization.
///
/// Requires GraphStore wired at Gateway startup.
pub async fn handle_query(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.query requires GraphStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.neighbors — returns neighbors of a node up to a given depth.
///
/// Requires GraphStore wired at Gateway startup.
pub async fn handle_neighbors(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.neighbors requires GraphStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.node_detail — returns full detail for a single node including wiki and facts.
///
/// Requires GraphStore wired at Gateway startup.
pub async fn handle_node_detail(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.node_detail requires GraphStore — wire in Gateway startup".to_string(),
    )
}

/// Handle graph.search — text search over node names and aliases.
///
/// Requires GraphStore wired at Gateway startup.
pub async fn handle_search(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "graph.search requires GraphStore — wire in Gateway startup".to_string(),
    )
}

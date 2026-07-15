use super::entry_to_dto;
use crate::gateway::handlers::graph_types::{
    GraphNeighborsParams, GraphNeighborsResponse, NoteLinkDto, NoteNodeDto,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;

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

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    let (entries, links, _truncated) = match db
        .get_neighbors(&params.node_id, agent_id, params.depth, params.limit)
        .await
    {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Look up the center node entry so the frontend can pin it at world origin.
    let center_entry = match db.get_note_index(&params.node_id, agent_id).await {
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
        .map(|(from, to)| NoteLinkDto {
            from,
            to,
            label: None,
            kind: None,
            confidence: None,
        })
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

/// Returns the hop distance from `center_id` to `target_id` for the radial
/// navigation view. Output is clamped to `{0, 1, 2}`:
///   - `0` if `target_id == center_id`
///   - `1` if any edge directly connects them
///   - `2` otherwise (any node further than 1 hop)
///
/// This is intentional: the radial view only renders two rings, so anything
/// beyond hop 1 is rendered on the outer ring regardless of true distance.
/// Callers that need true graph distance should not use this helper.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::graph::test_helpers::{make_db, make_note};
    use crate::memory::notes::store::NoteStore;

    fn neighbors_request(node_id: &str, depth: u8, limit: usize) -> JsonRpcRequest {
        neighbors_request_with_agent(node_id, depth, limit, None)
    }

    fn neighbors_request_with_agent(
        node_id: &str,
        depth: u8,
        limit: usize,
        agent_id: Option<&str>,
    ) -> JsonRpcRequest {
        let mut params = serde_json::json!({
            "node_id": node_id,
            "depth": depth,
            "limit": limit,
        });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.neighbors".to_string(),
            params: Some(params),
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
        // Index leaf targets BEFORE the notes that link to them: the links
        // resolver (tier 1, exact path) only marks a full-path wikilink
        // `status = 'active'` when the target already exists in the index at
        // write time, and `get_neighbors`'s edge list (used for hop_depth)
        // only surfaces active rows.
        let rust = make_note("Rust", "concept", vec!["concept/Cargo"]);
        let cargo = make_note("Cargo", "concept", vec!["concept/Clippy"]);
        let clippy = make_note("Clippy", "concept", vec![]);
        db.index_note(&clippy, agent, "concept").await.unwrap();
        db.index_note(&cargo, agent, "concept").await.unwrap();
        db.index_note(&rust, agent, "concept").await.unwrap();

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

        // concept/Cargo is directly linked from Rust → must be present and hop 1.
        // Use a hard expect() so the test fails (rather than silently passing)
        // if the BFS regresses and stops returning the direct neighbor.
        let h = resp
            .hop_depth
            .get("concept/Cargo")
            .copied()
            .expect("concept/Cargo must be in hop_depth");
        assert_eq!(h, 1, "concept/Cargo must be hop 1");
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
            label: None,
            kind: None,
            confidence: None,
        }];
        assert_eq!(compute_hop_depth("A", "B", &edges), 1);
        assert_eq!(compute_hop_depth("B", "A", &edges), 1); // reverse edge
        assert_eq!(compute_hop_depth("A", "C", &edges), 2); // not connected
        assert_eq!(compute_hop_depth("A", "A", &edges), 0); // self
    }

    #[tokio::test]
    async fn graph_neighbors_uses_explicit_agent_id() {
        let db = make_db();
        // Seed alpha with center→neighbor, beta with same center id but different neighbor
        let alpha_center = make_note("Hub", "concept", vec!["concept/AlphaPeer"]);
        let alpha_peer = make_note("AlphaPeer", "concept", vec![]);
        let beta_center = make_note("Hub", "concept", vec!["concept/BetaPeer"]);
        let beta_peer = make_note("BetaPeer", "concept", vec![]);
        db.index_note(&alpha_center, "alpha", "concept")
            .await
            .unwrap();
        db.index_note(&alpha_peer, "alpha", "concept")
            .await
            .unwrap();
        db.index_note(&beta_center, "beta", "concept")
            .await
            .unwrap();
        db.index_note(&beta_peer, "beta", "concept").await.unwrap();

        let req = neighbors_request_with_agent("concept/Hub", 2, 50, Some("alpha"));
        let resp = handle_neighbors_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphNeighborsResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let neighbor_ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            neighbor_ids.iter().any(|id| id.contains("AlphaPeer")),
            "alpha neighbor must appear: {neighbor_ids:?}"
        );
        assert!(
            !neighbor_ids.iter().any(|id| id.contains("BetaPeer")),
            "beta neighbor must NOT appear: {neighbor_ids:?}"
        );
    }

    #[tokio::test]
    async fn graph_neighbors_falls_back_to_default_agent_when_omitted() {
        let db = make_db();
        // Same Hub id under default and alpha — different neighbors. With
        // agent_id omitted, must return ONLY default's MainPeer, not alpha's
        // AlphaPeer. This proves (a) the fallback resolves to DEFAULT_AGENT_ID,
        // not (b) "returns all agents' neighbors".
        let main_center = make_note("Hub", "concept", vec!["concept/MainPeer"]);
        let main_peer = make_note("MainPeer", "concept", vec![]);
        let alpha_center = make_note("Hub", "concept", vec!["concept/AlphaPeer"]);
        let alpha_peer = make_note("AlphaPeer", "concept", vec![]);
        let agent = crate::routing::DEFAULT_AGENT_ID;
        db.index_note(&main_center, agent, "concept").await.unwrap();
        db.index_note(&main_peer, agent, "concept").await.unwrap();
        db.index_note(&alpha_center, "alpha", "concept")
            .await
            .unwrap();
        db.index_note(&alpha_peer, "alpha", "concept")
            .await
            .unwrap();

        let req = neighbors_request("concept/Hub", 2, 50); // no agent_id
        let resp = handle_neighbors_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphNeighborsResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let neighbor_ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            neighbor_ids.iter().any(|id| id.contains("MainPeer")),
            "default agent's neighbor must appear when agent_id omitted: {neighbor_ids:?}"
        );
        assert!(
            !neighbor_ids.iter().any(|id| id.contains("AlphaPeer")),
            "non-default agent's neighbor must NOT appear: {neighbor_ids:?}"
        );
    }
}

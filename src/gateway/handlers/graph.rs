//! Graph Query Handler
//!
//! Handles JSON-RPC requests for knowledge graph visualization.
//! These handlers are placeholders — actual GraphStore wiring happens at Gateway startup (Task 18).

use std::collections::{HashMap, HashSet};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::graph_types::{
    FactDto, GraphEdgeDto, GraphNeighborsParams, GraphNodeDetailParams, GraphNodeDto,
    GraphQueryParams, GraphQueryResponse, GraphSearchParams, GraphSearchResponse, GraphSearchResult,
    GraphNodeDetailResponse, WikiDto,
};
use crate::memory::context::FactType;
use crate::memory::store::{GraphStore, MemoryBackend, MemoryStore};

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

// ============================================================================
// Real implementation functions (wired in Task 18)
// ============================================================================

/// Real implementation of graph.query.
///
/// Returns nodes sorted by decay_score descending (up to `limit`), filtered
/// by `kind_filter` if non-empty, plus all edges connecting the returned nodes.
pub async fn handle_query_impl(
    req: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
    let params: GraphQueryParams = match serde_json::from_value(
        req.params.clone().unwrap_or(serde_json::Value::Object(Default::default())),
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            )
        }
    };

    // Fetch top nodes by decay score.
    let nodes = match db.get_high_score_nodes(0.0, params.limit * 10, &agent_id).await {
        Ok(n) => n,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("GraphStore error: {e}"),
            )
        }
    };

    // Apply kind filter and take up to limit.
    let filtered: Vec<_> = nodes
        .into_iter()
        .filter(|n| {
            params.kind_filter.is_empty() || params.kind_filter.contains(&n.kind)
        })
        .take(params.limit)
        .collect();

    // Collect node IDs for edge deduplication.
    let node_ids: HashSet<String> = filtered.iter().map(|n| n.id.clone()).collect();

    // Fetch edges and deduplicate.
    let mut seen_edges: HashSet<String> = HashSet::new();
    let mut edges: Vec<GraphEdgeDto> = Vec::new();
    let mut edge_count_map: HashMap<String, usize> = HashMap::new();

    for node in &filtered {
        let node_edges = match db.get_edges_for_node(&node.id, None, &agent_id).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        for edge in node_edges {
            // Only include edges where both endpoints are in the result set.
            if !node_ids.contains(&edge.from_id) || !node_ids.contains(&edge.to_id) {
                continue;
            }
            if seen_edges.insert(edge.id.clone()) {
                *edge_count_map.entry(edge.from_id.clone()).or_default() += 1;
                *edge_count_map.entry(edge.to_id.clone()).or_default() += 1;
                edges.push(GraphEdgeDto {
                    id: edge.id,
                    from_id: edge.from_id,
                    to_id: edge.to_id,
                    relation: edge.relation,
                    weight: edge.weight,
                    confidence: edge.confidence,
                });
            }
        }
    }

    // Build node DTOs, checking wiki presence.
    let mut node_dtos: Vec<GraphNodeDto> = Vec::with_capacity(filtered.len());
    for node in &filtered {
        let has_wiki = check_node_has_wiki(&db, &node.id, &agent_id).await;
        node_dtos.push(GraphNodeDto {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind.clone(),
            aliases: node.aliases.clone(),
            decay_score: node.decay_score,
            edge_count: *edge_count_map.get(&node.id).unwrap_or(&0),
            has_wiki,
        });
    }

    let response = GraphQueryResponse {
        nodes: node_dtos,
        edges,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Real implementation of graph.neighbors.
///
/// BFS from the given `node_id` up to `depth` hops, collecting up to `limit`
/// neighbour nodes and all edges between them.
pub async fn handle_neighbors_impl(
    req: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
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

    // BFS frontier.
    let mut visited: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = vec![params.node_id.clone()];
    visited.insert(params.node_id.clone());

    for _ in 0..params.depth {
        if visited.len() >= params.limit || frontier.is_empty() {
            break;
        }
        let mut next_frontier: Vec<String> = Vec::new();
        for node_id in &frontier {
            let edges = match db.get_edges_for_node(node_id, None, &agent_id).await {
                Ok(e) => e,
                Err(_) => continue,
            };
            for edge in edges {
                let neighbor = if edge.from_id == *node_id {
                    edge.to_id.clone()
                } else {
                    edge.from_id.clone()
                };
                if visited.len() < params.limit && visited.insert(neighbor.clone()) {
                    next_frontier.push(neighbor);
                }
            }
        }
        frontier = next_frontier;
    }

    // Fetch full node data for all discovered nodes.
    let mut node_dtos: Vec<GraphNodeDto> = Vec::new();
    let node_ids: HashSet<String> = visited.clone();

    for node_id in &visited {
        let node = match db.get_node(node_id, &agent_id).await {
            Ok(Some(n)) => n,
            _ => continue,
        };
        let has_wiki = check_node_has_wiki(&db, node_id, &agent_id).await;
        node_dtos.push(GraphNodeDto {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind.clone(),
            aliases: node.aliases.clone(),
            decay_score: node.decay_score,
            edge_count: 0, // filled below
            has_wiki,
        });
    }

    // Collect edges between the discovered nodes.
    let mut seen_edges: HashSet<String> = HashSet::new();
    let mut edges: Vec<GraphEdgeDto> = Vec::new();
    let mut edge_count_map: HashMap<String, usize> = HashMap::new();

    for node_id in &node_ids {
        let node_edges = match db.get_edges_for_node(node_id, None, &agent_id).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        for edge in node_edges {
            if !node_ids.contains(&edge.from_id) || !node_ids.contains(&edge.to_id) {
                continue;
            }
            if seen_edges.insert(edge.id.clone()) {
                *edge_count_map.entry(edge.from_id.clone()).or_default() += 1;
                *edge_count_map.entry(edge.to_id.clone()).or_default() += 1;
                edges.push(GraphEdgeDto {
                    id: edge.id,
                    from_id: edge.from_id,
                    to_id: edge.to_id,
                    relation: edge.relation,
                    weight: edge.weight,
                    confidence: edge.confidence,
                });
            }
        }
    }

    // Back-fill edge counts.
    for dto in &mut node_dtos {
        dto.edge_count = *edge_count_map.get(&dto.id).unwrap_or(&0);
    }

    let response = GraphQueryResponse {
        nodes: node_dtos,
        edges,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Real implementation of graph.node_detail.
///
/// Returns full node data, optional wiki fact, and all other linked facts.
pub async fn handle_node_detail_impl(
    req: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
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

    // Fetch the node.
    let node = match db.get_node(&params.node_id, &agent_id).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Node not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("GraphStore error: {e}"),
            )
        }
    };

    // Fetch linked fact IDs.
    let fact_links = match db.get_facts_for_node(&params.node_id, &agent_id).await {
        Ok(links) => links,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("get_facts_for_node error: {e}"),
            )
        }
    };

    // Fetch each fact and separate wiki from regular.
    let mut wiki_dto: Option<WikiDto> = None;
    let mut fact_dtos: Vec<FactDto> = Vec::new();

    for (fact_id, _weight) in &fact_links {
        let fact = match db.get_fact(fact_id).await {
            Ok(Some(f)) => f,
            _ => continue,
        };

        if fact.fact_type == FactType::Wiki {
            // Use the most recently updated wiki fact.
            let replace = wiki_dto
                .as_ref()
                .map(|w: &WikiDto| fact.updated_at > w.updated_at)
                .unwrap_or(true);
            if replace {
                wiki_dto = Some(WikiDto {
                    id: fact.id.clone(),
                    content: fact.content.clone(),
                    fact_source: fact.fact_source.to_string(),
                    updated_at: fact.updated_at,
                });
            }
        } else {
            fact_dtos.push(FactDto {
                id: fact.id.clone(),
                content: fact.content.clone(),
                confidence: fact.confidence,
                fact_type: fact.fact_type.as_str().to_string(),
            });
        }
    }

    let has_wiki = wiki_dto.is_some();
    let edge_count = db
        .get_edges_for_node(&params.node_id, None, &agent_id)
        .await
        .map(|e| e.len())
        .unwrap_or(0);

    let node_dto = GraphNodeDto {
        id: node.id,
        name: node.name,
        kind: node.kind,
        aliases: node.aliases,
        decay_score: node.decay_score,
        edge_count,
        has_wiki,
    };

    let response = GraphNodeDetailResponse {
        node: node_dto,
        wiki: wiki_dto,
        facts: fact_dtos,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

/// Real implementation of graph.search.
///
/// Uses `resolve_entity` to fuzzy-match node names and aliases.
pub async fn handle_search_impl(
    req: JsonRpcRequest,
    db: MemoryBackend,
    agent_id: String,
) -> JsonRpcResponse {
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

    let candidates = match db.resolve_entity(&params.query, None, &agent_id).await {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                format!("resolve_entity error: {e}"),
            )
        }
    };

    let results: Vec<GraphSearchResult> = candidates
        .into_iter()
        .take(params.limit)
        .map(|r| {
            // Determine which field matched: name or one of the aliases.
            let match_field = if r.name.to_lowercase().contains(&params.query.to_lowercase()) {
                "name".to_string()
            } else {
                "alias".to_string()
            };
            GraphSearchResult {
                id: r.node_id,
                name: r.name,
                kind: r.kind,
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

// ============================================================================
// Helpers
// ============================================================================

/// Returns true if any fact linked to `node_id` has `fact_type == Wiki`.
async fn check_node_has_wiki(db: &MemoryBackend, node_id: &str, agent_id: &str) -> bool {
    let links = match db.get_facts_for_node(node_id, agent_id).await {
        Ok(l) => l,
        Err(_) => return false,
    };
    for (fact_id, _) in &links {
        if let Ok(Some(fact)) = db.get_fact(fact_id).await {
            if fact.fact_type == FactType::Wiki {
                return true;
            }
        }
    }
    false
}

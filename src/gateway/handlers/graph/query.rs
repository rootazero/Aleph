use super::{entry_to_dto, undirected_key};
use crate::gateway::handlers::graph_types::{
    GraphQueryParams, GraphQueryResponse, NoteLinkDto, NoteNodeDto,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;

/// Real implementation of graph.query.
///
/// Returns notes sorted by `link_count` + recency (up to `limit`),
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

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    // P1 partition isolation (spec §11-1c): an invisible partition reads as
    // an empty graph — the same shape a genuinely unused agent_id produces —
    // without ever touching the store under the caller's chosen name (no
    // existence oracle).
    if !crate::gateway::visibility::partition_visible(agent_id) {
        let response = GraphQueryResponse {
            nodes: vec![],
            edges: vec![],
            total: Some(0),
            bridge_nodes: vec![],
            surprising_edges: vec![],
        };
        return match serde_json::to_value(response) {
            Ok(v) => JsonRpcResponse::success(req.id, v),
            Err(e) => {
                JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}"))
            }
        };
    }

    let (entries, links) = match db.get_graph_data(agent_id, params.limit).await {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Community map (cold cache => empty) + agent-scoped total for truncation.
    let communities = db.community_ids(agent_id).await.unwrap_or_default();
    let total = db
        .count_notes(agent_id)
        .await
        .ok()
        .map(|t| t.max(0) as usize);

    let nodes: Vec<NoteNodeDto> = entries
        .iter()
        .map(|e| {
            let mut dto = entry_to_dto(e);
            dto.community_id = communities.get(&e.path).map(|&c| c.max(0) as u32);
            dto
        })
        .collect();
    let edges: Vec<NoteLinkDto> = links
        .into_iter()
        .map(|row| NoteLinkDto {
            from: row.from,
            to: row.to,
            label: row.label,
            // NULL relation = plain body wikilink.
            kind: Some(row.relation.unwrap_or_else(|| "wikilink".to_string())),
            confidence: Some(row.confidence),
        })
        .collect();

    // Similarity edges (5-signal + MinHash, materialized by GraphRecompute) —
    // top-3 per node, deduped against real links by undirected pair.
    let visible: std::collections::HashSet<String> =
        entries.iter().map(|e| e.path.clone()).collect();
    let mut seen: std::collections::HashSet<(String, String)> = edges
        .iter()
        .map(|e| undirected_key(&e.from, &e.to))
        .collect();
    let mut edges = edges;
    if let Ok(related) = db.related_edges_between(agent_id, &visible, 3).await {
        for (from, to, _score) in related {
            let key = undirected_key(&from, &to);
            if seen.insert(key) {
                edges.push(NoteLinkDto {
                    from,
                    to,
                    label: None,
                    kind: Some("related_similarity".to_string()),
                    confidence: None,
                });
            }
        }
    }

    // Graph-health emphasis payloads (bridge nodes + surprising edges).
    // `sparse` stays orientation-only by design (spec S3).
    let bridge_nodes: Vec<String> = db
        .read_graph_insights(agent_id, Some("bridge"))
        .await
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .find_map(|(_, p)| serde_json::from_str::<Vec<String>>(&p).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|p| visible.contains(p))
        .collect();
    #[derive(serde::Deserialize)]
    struct SurprisingRow {
        from: String,
        to: String,
    }
    let surprising_edges: Vec<(String, String)> = db
        .read_graph_insights(agent_id, Some("surprising"))
        .await
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .find_map(|(_, p)| serde_json::from_str::<Vec<SurprisingRow>>(&p).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|r| visible.contains(&r.from) && visible.contains(&r.to))
        .map(|r| (r.from, r.to))
        .collect();

    let response = GraphQueryResponse {
        nodes,
        edges,
        total,
        bridge_nodes,
        surprising_edges,
    };

    match serde_json::to_value(response) {
        Ok(v) => JsonRpcResponse::success(req.id, v),
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::graph::test_helpers::{make_db, make_note};
    use crate::memory::notes::store::NoteStore;

    /// Seed `db` with one note per agent. Returns (alpha_path, beta_path).
    async fn seed_two_agents(db: &MemoryBackend) -> (String, String) {
        let alpha_note = make_note("AlphaOnly", "concept", vec![]);
        let beta_note = make_note("BetaOnly", "concept", vec![]);
        db.index_note(&alpha_note, "alpha", "concept")
            .await
            .unwrap();
        db.index_note(&beta_note, "beta", "concept").await.unwrap();
        (
            "concept/AlphaOnly".to_string(),
            "concept/BetaOnly".to_string(),
        )
    }

    fn query_request(limit: usize, agent_id: Option<&str>) -> JsonRpcRequest {
        let params = match agent_id {
            Some(id) => serde_json::json!({ "limit": limit, "agent_id": id }),
            None => serde_json::json!({ "limit": limit }),
        };
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.query".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn graph_query_uses_explicit_agent_id() {
        let (_scratch, db) = make_db();
        let (alpha_path, _beta_path) = seed_two_agents(&db).await;

        let req = query_request(50, Some("alpha"));
        let resp = handle_query_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphQueryResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&alpha_path.as_str()),
            "alpha note must appear: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.contains("BetaOnly")),
            "beta note must NOT appear when querying alpha: {ids:?}"
        );

        // Enriched fields (Plan 2): agent-scoped total, per-node updated_at,
        // per-edge kind (plain wikilinks map to "wikilink").
        assert!(result.total.is_some(), "total should be populated");
        assert!(
            result.nodes.iter().all(|n| n.updated_at.is_some()),
            "every node carries updated_at"
        );
        assert!(
            result.edges.iter().all(|e| e.kind.is_some()),
            "every edge carries a kind"
        );
    }

    #[tokio::test]
    async fn graph_query_falls_back_to_default_agent_when_omitted() {
        let (_scratch, db) = make_db();
        // Seed both default agent and a non-default agent. When agent_id is
        // omitted, the handler must return ONLY the default agent's notes —
        // proving (a) it falls back to DEFAULT_AGENT_ID, not (b) returns all
        // agents' notes.
        let main_note = make_note("MainNote", "concept", vec![]);
        db.index_note(&main_note, crate::routing::DEFAULT_AGENT_ID, "concept")
            .await
            .unwrap();
        let alpha_note = make_note("AlphaOnly", "concept", vec![]);
        db.index_note(&alpha_note, "alpha", "concept")
            .await
            .unwrap();

        let req = query_request(50, None);
        let resp = handle_query_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphQueryResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.iter().any(|id| id.contains("MainNote")),
            "default agent's note must appear when agent_id omitted: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.contains("AlphaOnly")),
            "non-default agent's note must NOT appear when agent_id omitted: {ids:?}"
        );
    }

    #[tokio::test]
    async fn graph_query_carries_similarity_edges_and_insights() {
        let (_scratch, db) = make_db();
        let agent = crate::routing::DEFAULT_AGENT_ID;
        let a = make_note("A", "concept", vec![]);
        let b = make_note("B", "concept", vec![]);
        db.index_note(&a, agent, "concept").await.unwrap();
        db.index_note(&b, agent, "concept").await.unwrap();
        // Materialized artifacts (what GraphRecompute would write).
        db.replace_graph_related(agent, &[("concept/A".into(), "concept/B".into(), 3.2)])
            .await
            .unwrap();
        db.replace_graph_insights(
            agent,
            &[
                (
                    "bridge".into(),
                    serde_json::json!(["concept/A"]).to_string(),
                ),
                (
                    "surprising".into(),
                    serde_json::json!([{"from": "concept/A", "to": "concept/B", "score": 0.9}])
                        .to_string(),
                ),
            ],
        )
        .await
        .unwrap();

        let resp = handle_query_impl(query_request(50, Some(agent)), db).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result: GraphQueryResponse = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(
            result
                .edges
                .iter()
                .any(|e| e.kind.as_deref() == Some("related_similarity")),
            "similarity edge must surface: {:?}",
            result.edges
        );
        assert_eq!(result.bridge_nodes, vec!["concept/A"]);
        assert_eq!(
            result.surprising_edges,
            vec![("concept/A".into(), "concept/B".into())]
        );
    }

    /// P1 partition isolation: bob addressing alice's personal partition by
    /// name gets an empty graph — the same shape an unknown agent_id
    /// produces — not alice's real notes.
    #[tokio::test]
    async fn foreign_partition_reads_an_empty_graph_not_the_owners_notes() {
        use crate::gateway::caller_identity::CALLER_USER;

        let (_scratch, db) = make_db();
        let secret = make_note("AliceSecret", "concept", vec![]);
        db.index_note(&secret, "main__u-alice", "concept")
            .await
            .unwrap();

        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_query_impl(query_request(50, Some("main__u-alice")), db).await
            })
            .await;
        assert!(
            resp.error.is_none(),
            "success, not an error: {:?}",
            resp.error
        );
        let result: GraphQueryResponse = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(result.nodes.is_empty(), "bob must not see alice's notes");
        assert_eq!(result.total, Some(0));
        assert!(result.edges.is_empty());
        assert!(result.bridge_nodes.is_empty());
        assert!(result.surprising_edges.is_empty());
    }
}

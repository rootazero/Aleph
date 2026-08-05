use super::{entry_to_dto, notes_dir};
use crate::gateway::handlers::graph_types::{
    GraphNodeDetailParams, NoteDetailResponse, OutgoingLinkDto,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;

/// Real implementation of `graph.node_detail`.
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

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    // P1 partition isolation (spec §11-1c): an invisible partition gets the
    // exact same "not found" response a nonexistent node would — no oracle
    // distinguishing "this note doesn't exist" from "you can't see it" —
    // without ever reading its (potentially full-content) row.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!("Note not found: {}", params.node_id),
        );
    }

    // Fetch the note index entry.
    let entry = match db.get_note_index(&params.node_id, agent_id).await {
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
    let md_path = notes_dir()
        .join(agent_id)
        .join(format!("{}.md", entry.path));
    let content = tokio::fs::read_to_string(&md_path)
        .await
        .unwrap_or_default();

    // Fetch backlinks (incoming links).
    let backlinks = db
        .get_incoming_links(&params.node_id, agent_id)
        .await
        .unwrap_or_default();

    // Outgoing links with full lifecycle provenance (active/dangling/tombstone),
    // unlike the graph feed which only surfaces active edges.
    let outgoing: Vec<OutgoingLinkDto> = db
        .get_outgoing_link_rows(&params.node_id, agent_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| OutgoingLinkDto {
            to: r.to_note,
            raw: r.to_raw,
            relation: r.relation,
            label: r.label,
            confidence: r.confidence,
            resolved_by: r.resolved_by,
            status: r.status,
        })
        .collect();

    let node = entry_to_dto(&entry);
    let response = NoteDetailResponse {
        node,
        content,
        backlinks,
        outgoing,
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
    use crate::memory::notes::KnowledgeNote;

    fn node_detail_request(node_id: &str, agent_id: Option<&str>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "node_id": node_id });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.node_detail".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn graph_node_detail_uses_explicit_agent_id() {
        let db = make_db();
        // Same id under two agents, different content_hash — proves we got alpha's row.
        let alpha = KnowledgeNote {
            content_hash: "alpha_hash".to_string(),
            ..make_note("Shared", "concept", vec![])
        };
        let beta = KnowledgeNote {
            content_hash: "beta_hash".to_string(),
            ..make_note("Shared", "concept", vec![])
        };
        db.index_note(&alpha, "alpha", "concept").await.unwrap();
        db.index_note(&beta, "beta", "concept").await.unwrap();

        let req = node_detail_request("concept/Shared", Some("alpha"));
        let resp = handle_node_detail_impl(req, db).await;
        // The note exists in alpha — handler must succeed (not return "not found").
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    }

    #[tokio::test]
    async fn graph_node_detail_returns_not_found_for_other_agent() {
        let db = make_db();
        // Note exists in alpha but we ask for beta → must be 'not found'.
        let alpha = make_note("AlphaOnly", "concept", vec![]);
        db.index_note(&alpha, "alpha", "concept").await.unwrap();

        let req = node_detail_request("concept/AlphaOnly", Some("beta"));
        let resp = handle_node_detail_impl(req, db).await;
        assert!(
            resp.error.is_some(),
            "expected error for cross-agent lookup"
        );
    }

    #[tokio::test]
    async fn graph_node_detail_falls_back_to_default_agent_when_omitted() {
        let db = make_db();
        // Seed: MainOnly under DEFAULT_AGENT_ID, AlphaOnly under alpha.
        // With agent_id omitted, MainOnly must be findable AND AlphaOnly must NOT.
        // This proves the fallback resolves to DEFAULT_AGENT_ID (not "any agent").
        let main = make_note("MainOnly", "concept", vec![]);
        let alpha = make_note("AlphaOnly", "concept", vec![]);
        db.index_note(&main, crate::routing::DEFAULT_AGENT_ID, "concept")
            .await
            .unwrap();
        db.index_note(&alpha, "alpha", "concept").await.unwrap();

        // Default-owned note must be findable when agent_id omitted.
        let req_main = node_detail_request("concept/MainOnly", None);
        let resp_main = handle_node_detail_impl(req_main, db.clone()).await;
        assert!(
            resp_main.error.is_none(),
            "default note must be reachable when agent_id omitted: {:?}",
            resp_main.error
        );

        // Alpha-owned note must NOT be findable via fallback.
        let req_alpha = node_detail_request("concept/AlphaOnly", None);
        let resp_alpha = handle_node_detail_impl(req_alpha, db).await;
        assert!(
            resp_alpha.error.is_some(),
            "alpha-only note must NOT be reachable when agent_id omitted"
        );
    }

    #[tokio::test]
    async fn node_detail_lists_outgoing_with_provenance() {
        let db = make_db();
        let agent = crate::routing::DEFAULT_AGENT_ID;
        db.index_note(&make_note("t", "concept", vec![]), agent, "concept")
            .await
            .unwrap();
        db.index_note(
            &make_note("s", "concept", vec!["concept/t", "ghost"]),
            agent,
            "concept",
        )
        .await
        .unwrap();
        let resp = handle_node_detail_impl(node_detail_request("concept/s", Some(agent)), db).await;
        let v = resp.result.unwrap();
        let outgoing = v.get("outgoing").unwrap().as_array().unwrap();
        assert_eq!(outgoing.len(), 2);
        let ghost = outgoing.iter().find(|o| o["raw"] == "ghost").unwrap();
        assert_eq!(ghost["status"], "dangling");
    }

    /// P1 partition isolation: bob reading alice's note by its real
    /// node_id gets the same "not found" response a nonexistent node
    /// produces — no oracle, and the full markdown body is never read.
    #[tokio::test]
    async fn foreign_partition_denies_with_the_not_found_shape() {
        use crate::gateway::caller_identity::CALLER_USER;

        let db = make_db();
        let secret = make_note("AliceSecret", "concept", vec![]);
        db.index_note(&secret, "main__u-alice", "concept")
            .await
            .unwrap();

        let owned = node_detail_request("concept/AliceSecret", Some("main__u-alice"));
        let deny_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_node_detail_impl(owned, db).await
            })
            .await;
        let deny_err = deny_resp.error.expect("must be denied");

        // Same (node_id, agent_id) pair, compared against a FRESH store
        // where it genuinely never existed — any difference in the message
        // can only come from the denial itself, not from the node_id
        // appearing in the text.
        let empty_db = make_db();
        let missing_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_node_detail_impl(
                    node_detail_request("concept/AliceSecret", Some("main__u-alice")),
                    empty_db,
                )
                .await
            })
            .await;
        let missing_err = missing_resp
            .error
            .expect("genuinely missing must error too");

        assert_eq!(
            deny_err.message, missing_err.message,
            "denied and genuinely-missing must be byte-identical (no oracle)"
        );
    }
}

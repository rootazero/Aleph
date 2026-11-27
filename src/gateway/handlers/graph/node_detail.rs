use super::{entry_to_dto, notes_dir};
use crate::gateway::handlers::graph_types::{
    GraphNodeDetailParams, NoteAddress, NoteDetailResponse, OutgoingLinkDto,
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
    let (content, revision) = match tokio::fs::read_to_string(&md_path).await {
        Ok(content) => {
            let revision = crate::memory::notes::indexer::note_revision(&content);
            (content, Some(revision))
        }
        Err(_) => (String::new(), None),
    };

    // Fetch backlinks (incoming links).
    let backlinks = db
        .get_incoming_links(&params.node_id, agent_id)
        .await
        .unwrap_or_default();

    // Outgoing links with full lifecycle provenance (active/dangling/tombstone),
    // unlike the graph feed which only surfaces active edges.
    let rows = db
        .get_outgoing_link_rows(&params.node_id, agent_id)
        .await
        .unwrap_or_default();
    let mut outgoing = Vec::new();
    for r in rows {
        let target = if r.status == "active" {
            db.get_note_index(&r.to_note, agent_id)
                .await
                .ok()
                .flatten()
                .map(|e| NoteAddress {
                    agent_id: e.agent_id,
                    node_id: e.path,
                })
        } else {
            None
        };
        outgoing.push(OutgoingLinkDto {
            target,
            to: r.to_note,
            raw: r.to_raw,
            relation: r.relation,
            label: r.label,
            confidence: r.confidence,
            resolved_by: r.resolved_by,
            status: r.status,
        });
    }
    let mut backlink_targets = Vec::new();
    for path in &backlinks {
        let active = db
            .get_outgoing_link_rows(path, agent_id)
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| r.status == "active" && r.to_note == params.node_id);
        if !active {
            continue;
        }
        if let Some(e) = db.get_note_index(path, agent_id).await.ok().flatten() {
            backlink_targets.push(NoteAddress {
                agent_id: e.agent_id,
                node_id: e.path,
            });
        }
    }

    backlink_targets.sort_by(|a, b| (&a.agent_id, &a.node_id).cmp(&(&b.agent_id, &b.node_id)));
    backlink_targets.dedup();
    let node = entry_to_dto(&entry);
    let response = NoteDetailResponse {
        backlink_targets: Some(backlink_targets),
        node,
        content,
        revision,
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
        let (_scratch, db) = make_db();
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
        let (_scratch, db) = make_db();
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
        let (_scratch, db) = make_db();
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
        let (_scratch, db) = make_db();
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

    #[tokio::test]
    async fn exact_relation_targets_require_active_extant_endpoints() {
        let (_scratch, db) = make_db();
        for agent in ["alpha", "beta"] {
            for (name, links) in [
                ("live", vec![]),
                ("dead", vec![]),
                ("root", vec!["concept/live", "concept/dead", "ghost"]),
                ("source", vec!["concept/root"]),
            ] {
                db.index_note(&make_note(name, "concept", links), agent, "concept")
                    .await
                    .unwrap();
            }
            db.remove_note_index("concept/dead", agent).await.unwrap();
        }
        db.add_link_with_relation("alpha", "concept/root", "concept/missing", "refers")
            .await
            .unwrap();
        db.add_link_with_relation("alpha", "concept/missing_source", "concept/root", "refers")
            .await
            .unwrap();
        let response =
            handle_node_detail_impl(node_detail_request("concept/root", Some("alpha")), db).await;
        let value = response.result.unwrap();
        let mut backlinks: Vec<_> = value["backlinks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        backlinks.sort_unstable();
        assert_eq!(backlinks, ["concept/missing_source", "concept/source"]);
        assert_eq!(
            value["backlink_targets"],
            serde_json::json!([{"agent_id":"alpha", "node_id":"concept/source"}])
        );
        let mut legacy = value.clone();
        legacy.as_object_mut().unwrap().remove("backlink_targets");
        for row in legacy["outgoing"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("target");
        }
        let legacy: NoteDetailResponse = serde_json::from_value(legacy).unwrap();
        assert!(legacy.backlink_targets.is_none());
        assert!(legacy.outgoing.iter().all(|r| r.target.is_none()));
        let outgoing = value["outgoing"].as_array().unwrap();
        assert_eq!(outgoing.len(), 4);
        for row in outgoing {
            if row["to"] == "concept/live" {
                assert_eq!(
                    row["target"],
                    serde_json::json!({"agent_id":"alpha", "node_id":"concept/live"})
                );
            } else {
                assert!(row.get("target").is_none());
            }
        }
    }

    /// P1 partition isolation: bob reading alice's note by its real
    /// node_id gets the same "not found" response a nonexistent node
    /// produces — no oracle, and the full markdown body is never read.
    #[tokio::test]
    async fn foreign_partition_denies_with_the_not_found_shape() {
        use crate::gateway::caller_identity::CALLER_USER;

        let (_scratch, db) = make_db();
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
        let (_scratch, empty_db) = make_db();
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

use crate::gateway::handlers::graph_types::{
    GraphSearchParams, GraphSearchResponse, SearchResultDto,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;

/// Real implementation of graph.search.
///
/// Full-text search over note content via `NoteStore` FTS index.
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

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    // P1 partition isolation (spec §11-1c): an invisible partition reads as
    // a no-hits search — the same shape a genuinely unused partition
    // produces — without ever running the FTS query under the caller's
    // chosen name (no oracle, no title/tag leak).
    if !crate::gateway::visibility::partition_visible(agent_id) {
        let response = GraphSearchResponse { results: vec![] };
        return match serde_json::to_value(response) {
            Ok(v) => JsonRpcResponse::success(req.id, v),
            Err(e) => {
                JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("Serialize error: {e}"))
            }
        };
    }

    let entries = match db
        .search_notes_fts(&params.query, agent_id, params.limit)
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
            // Match-field heuristic: did the query hit the filename?
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
                agent_id: entry.agent_id,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                tags: entry.tags,
                link_count: entry.link_count,
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
    use crate::gateway::handlers::graph::test_helpers::{make_db, make_note};
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;

    fn search_request(query: &str, limit: usize, agent_id: Option<&str>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "query": query, "limit": limit });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.search".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    /// Build a note whose facts contain a single token, so FTS5 (unicode61
    /// tokenizer) can match the query as a whole word. The unicode61 tokenizer
    /// splits on non-alphanumerics, so multi-word filenames like "AlphaUnique"
    /// are a single token and won't match a phrase search for "Unique".
    fn make_note_with_fact(title: &str, fact: &str) -> KnowledgeNote {
        let mut note = make_note(title, "concept", vec![]);
        note.facts = vec![fact.to_string()];
        note
    }

    #[tokio::test]
    async fn graph_search_uses_explicit_agent_id() {
        let (_scratch, db) = make_db();
        // Both agents have the SAME fact word so the FTS query alone cannot
        // distinguish them. Only an agent_id filter on the handler can. We
        // distinguish in the assertion by note title.
        let alpha = make_note_with_fact("AlphaSearchNote", "sharedfactword");
        let beta = make_note_with_fact("BetaSearchNote", "sharedfactword");
        db.index_note(&alpha, "alpha", "concept").await.unwrap();
        db.index_note(&beta, "beta", "concept").await.unwrap();

        let req = search_request("sharedfactword", 20, Some("alpha"));
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphSearchResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let names: Vec<&str> = result.results.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("AlphaSearchNote")),
            "alpha hit must appear: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("BetaSearchNote")),
            "beta hit must NOT appear when querying alpha: {names:?}"
        );
    }

    #[tokio::test]
    async fn graph_search_falls_back_to_default_agent_when_omitted() {
        let (_scratch, db) = make_db();
        // Strengthened: seed both DEFAULT and alpha with a shared query word.
        // With agent_id omitted, must return ONLY default's hit, proving
        // fallback resolves to DEFAULT_AGENT_ID and does not leak across agents.
        let main = make_note_with_fact("MainNote", "sharedsearchword");
        let alpha = make_note_with_fact("AlphaNote", "sharedsearchword");
        db.index_note(&main, crate::routing::DEFAULT_AGENT_ID, "concept")
            .await
            .unwrap();
        db.index_note(&alpha, "alpha", "concept").await.unwrap();

        let req = search_request("sharedsearchword", 20, None);
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
        let result: GraphSearchResponse =
            serde_json::from_value(resp.result.expect("result")).expect("deserialize");

        let names: Vec<&str> = result.results.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("MainNote")),
            "default agent's hit must appear when agent_id omitted: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("AlphaNote")),
            "non-default agent's hit must NOT appear: {names:?}"
        );
    }

    /// The SearchHits layer renders these as note cards, so a hit must carry
    /// everything a card shows. All of it is already on NoteIndexEntry.
    #[tokio::test]
    async fn search_hits_carry_full_note_row() {
        let (_scratch, db) = make_db();
        let mut note = make_note_with_fact("TaggedSearchNote", "distinctivefactword");
        note.tags = vec!["rust".to_string(), "ci".to_string()];
        db.index_note(&note, "main", "concept").await.unwrap();

        let req = search_request("distinctivefactword", 20, Some("main"));
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);

        let v = resp.result.expect("result");
        let hit = &v["results"][0];
        assert_eq!(hit["agent_id"], "main");
        assert!(hit["created_at"].is_i64(), "created_at must be present");
        assert!(hit["updated_at"].is_i64(), "updated_at must be present");
        assert!(hit["link_count"].is_u64(), "link_count must be present");
        let tags: Vec<String> = serde_json::from_value(hit["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["rust".to_string(), "ci".to_string()]);
    }

    /// P1 partition isolation: bob searching alice's partition by name gets
    /// no hits — the same shape a genuinely unused partition produces — not
    /// alice's titles/tags/content.
    #[tokio::test]
    async fn foreign_partition_search_returns_no_hits_not_the_owners_notes() {
        use crate::gateway::caller_identity::CALLER_USER;

        let (_scratch, db) = make_db();
        let secret = make_note_with_fact("AliceSecretNote", "distinctivesearchword");
        db.index_note(&secret, "main__u-alice", "concept")
            .await
            .unwrap();

        let req = search_request("distinctivesearchword", 20, Some("main__u-alice"));
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_search_impl(req, db).await
            })
            .await;
        assert!(
            resp.error.is_none(),
            "success, not an error: {:?}",
            resp.error
        );
        let result: GraphSearchResponse = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(
            result.results.is_empty(),
            "bob must not see alice's search hits: {:?}",
            result.results
        );
    }
}

use crate::gateway::handlers::graph_types::{
    GraphDeleteNoteParams, GraphRenameNoteParams, GraphUpdateNoteParams,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::Arc;

/// Real implementation of `graph.update_note`.
///
/// Persists the edited markdown for a single note verbatim (via
/// `NoteIndexer::write_note_raw`, which writes byte-for-byte and re-indexes),
/// so hand-edited prose / headings / code blocks survive — unlike the lossy
/// `KnowledgeNote::to_markdown` reconstruction. Last-write-wins.
pub async fn handle_update_note_impl(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphUpdateNoteParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required params: node_id, content".to_string(),
            )
        }
    };

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);

    // node_id is the note path `"category/title"`. Split on the first '/' —
    // categories are flat (see CATEGORY_DIRS) and titles never contain '/'.
    let Some((category, title)) = params.node_id.split_once('/') else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!(
                "Invalid node_id (expected \"category/title\"): {}",
                params.node_id
            ),
        );
    };

    // Defensive (P7): `agent_id` and `category` are joined verbatim into the
    // on-disk note path by `write_note_raw` (only `title` is sanitized there),
    // so a crafted node_id like "../x" or an agent_id like "../../etc" would
    // write outside the notes directory. Reject traversal components here.
    if category.contains("..")
        || category.contains('\\')
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "node_id / agent_id must not contain path traversal components".to_string(),
        );
    }

    match indexer
        .write_note_raw(agent_id, category, title, &params.content)
        .await
    {
        Ok(_path) => JsonRpcResponse::success(
            req.id,
            serde_json::json!({ "node_id": params.node_id, "saved": true }),
        ),
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("update_note failed: {e}"))
        }
    }
}

/// Real implementation of `graph.rename_note`: renames the file, rewrites
/// every inbound `[[old]]` wikilink, re-indexes, and backfills.
pub async fn handle_rename_note_impl(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphRenameNoteParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required params: node_id, new_title".to_string(),
            )
        }
    };
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let Some((category, title)) = params.node_id.split_once('/') else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!(
                "Invalid node_id (expected \"category/title\"): {}",
                params.node_id
            ),
        );
    };
    if category.contains("..")
        || category.contains('\\')
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "node_id / agent_id must not contain path traversal components".to_string(),
        );
    }
    match indexer
        .rename_note(agent_id, title, &params.new_title)
        .await
    {
        Ok(()) => {
            // `rename_note` ignores the client-supplied `category` prefix and
            // re-derives the real one via `find_by_filename(old_title, ..)`
            // internally — so if the client's category was stale/wrong, the
            // rename still succeeds against the real file, but the naive
            // `{category}/{new_title}` reconstruction below would point at a
            // path that was never written. Look up the canonical new path the
            // same way note_manage's `handle_rename` does (Task 7's tool
            // layer), falling back to the client-category form only if the
            // lookup comes up empty. NOTE (cross-category title collision):
            // if two categories both contain a note with `new_title`, this
            // returns the first hit from `find_by_filename`, which may not be
            // the one that was just renamed — same caveat as note_manage.
            let new_paths = indexer
                .store()
                .find_by_filename(&params.new_title, agent_id)
                .await
                .unwrap_or_default();
            let new_id = new_paths
                .first()
                .cloned()
                .unwrap_or_else(|| format!("{category}/{}", params.new_title));
            JsonRpcResponse::success(
                req.id,
                serde_json::json!({
                    "node_id": params.node_id,
                    "new_id": new_id,
                    "renamed": true
                }),
            )
        }
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("rename_note failed: {e}"))
        }
    }
}

/// Real implementation of `graph.delete_note`: removes file + index; inbound
/// links become tombstones (D1 — source bodies untouched, revivable).
pub async fn handle_delete_note_impl(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphDeleteNoteParams = match req
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
    let Some((category, title)) = params.node_id.split_once('/') else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!(
                "Invalid node_id (expected \"category/title\"): {}",
                params.node_id
            ),
        );
    };
    if category.contains("..")
        || category.contains('\\')
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "node_id / agent_id must not contain path traversal components".to_string(),
        );
    }
    match indexer.delete_note(agent_id, category, title).await {
        Ok(()) => JsonRpcResponse::success(
            req.id,
            serde_json::json!({ "node_id": params.node_id, "deleted": true }),
        ),
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("delete_note failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::graph::test_helpers::make_db;
    use crate::memory::notes::store::NoteStore;
    use uuid::Uuid;

    fn update_note_request(node_id: &str, content: &str, agent_id: Option<&str>) -> JsonRpcRequest {
        let mut params = serde_json::json!({ "node_id": node_id, "content": content });
        if let Some(id) = agent_id {
            params["agent_id"] = serde_json::Value::String(id.to_string());
        }
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "graph.update_note".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    /// update_note persists the body VERBATIM — prose / headings that are not
    /// bullet facts must survive (proving we write raw, not via the lossy
    /// `KnowledgeNote::to_markdown` reconstruction that only re-emits bullets).
    #[tokio::test]
    async fn update_note_persists_content_verbatim() {
        let memory_dir = std::env::temp_dir().join(format!("update_note_test_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));

        let agent = crate::routing::DEFAULT_AGENT_ID;
        let content = "---\ncategory: reference\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n# A Heading\n\nProse that is not a bullet fact.\n\n- a bullet fact\n";

        let req = update_note_request("reference/MyNote", content, Some(agent));
        let resp = handle_update_note_impl(req, indexer).await;
        assert!(resp.error.is_none(), "update_note failed: {:?}", resp.error);

        // File written verbatim — the heading + prose survive (to_markdown drops them).
        let path = memory_dir.join(agent).join("reference").join("MyNote.md");
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(written, content, "content must round-trip byte-for-byte");

        // And it is indexed so the graph reflects the edit without a full rebuild.
        let entry = db.get_note_index("reference/MyNote", agent).await.unwrap();
        assert!(entry.is_some(), "note must be indexed after update_note");
    }

    /// A node_id without a `category/` prefix is rejected with an error.
    #[tokio::test]
    async fn update_note_rejects_node_id_without_category() {
        let memory_dir = std::env::temp_dir().join(format!("update_note_test_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir, db));

        let req = update_note_request("NoCategory", "body", Some("default"));
        let resp = handle_update_note_impl(req, indexer).await;
        assert!(
            resp.error.is_some(),
            "expected error for category-less node_id"
        );
    }

    #[tokio::test]
    async fn rename_note_moves_file_and_reindexes() {
        let memory_dir = std::env::temp_dir().join(format!("rename_rpc_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));
        let agent = crate::routing::DEFAULT_AGENT_ID;
        // Seed a real on-disk note through the indexer write path.
        indexer
            .write_note_raw(agent, "reference", "OldTitle",
                "---\ncategory: reference\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- fact\n")
            .await
            .unwrap();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.rename_note".into(),
            params: Some(serde_json::json!({
                "node_id": "reference/OldTitle", "new_title": "NewTitle", "agent_id": agent })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_rename_note_impl(req, indexer).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        assert!(memory_dir
            .join(agent)
            .join("reference/NewTitle.md")
            .exists());
        assert!(!memory_dir
            .join(agent)
            .join("reference/OldTitle.md")
            .exists());
        assert!(db
            .get_note_index("reference/NewTitle", agent)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn delete_note_removes_file_and_index() {
        let memory_dir = std::env::temp_dir().join(format!("delete_rpc_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));
        let agent = crate::routing::DEFAULT_AGENT_ID;
        indexer
            .write_note_raw(agent, "plan", "Doomed",
                "---\ncategory: plan\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- x\n")
            .await
            .unwrap();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.delete_note".into(),
            params: Some(serde_json::json!({ "node_id": "plan/Doomed", "agent_id": agent })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_delete_note_impl(req, indexer).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        assert!(!memory_dir.join(agent).join("plan/Doomed.md").exists());
        assert!(db
            .get_note_index("plan/Doomed", agent)
            .await
            .unwrap()
            .is_none());
    }

    /// Traversal in the category segment of node_id (e.g. "../evil/Note")
    /// must be rejected by the same guard as `update_note`'s, not silently
    /// forwarded to `rename_note`. Asserts the specific error code + message
    /// (not just `is_err`) so the test regresses if the guard is ever removed
    /// or weakened rather than passing vacuously on an unrelated error.
    #[tokio::test]
    async fn rename_note_rejects_traversal_category() {
        let memory_dir =
            std::env::temp_dir().join(format!("rename_rpc_traversal_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir, db));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.rename_note".into(),
            params: Some(serde_json::json!({
                "node_id": "../evil/Note", "new_title": "NewTitle"
            })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_rename_note_impl(req, indexer).await;
        let err = resp.error.expect("expected error for traversal category");
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(
            err.message.contains("path traversal"),
            "expected traversal guard message, got: {}",
            err.message
        );
    }

    /// Same traversal guard, for `graph.delete_note`.
    #[tokio::test]
    async fn delete_note_rejects_traversal_category() {
        let memory_dir =
            std::env::temp_dir().join(format!("delete_rpc_traversal_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir, db));

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.delete_note".into(),
            params: Some(serde_json::json!({ "node_id": "../evil/Note" })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_delete_note_impl(req, indexer).await;
        let err = resp.error.expect("expected error for traversal category");
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(
            err.message.contains("path traversal"),
            "expected traversal guard message, got: {}",
            err.message
        );
    }
}

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

    // P1 partition isolation (spec §11-1c): `update_note` is upsert
    // (create-or-overwrite) with no "not found" error of its own — a
    // nonexistent node_id under a VISIBLE partition simply creates it,
    // reporting success (see this fn's doc). The no-oracle-consistent
    // denial for an INVISIBLE partition is that SAME success shape, without
    // ever touching the store: a caller who cannot see this partition sees
    // exactly what they would see writing a brand-new note anywhere else,
    // and nothing is actually written under a partition they don't own.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(
            req.id,
            serde_json::json!({ "node_id": params.node_id, "saved": true }),
        );
    }

    // Hard security floor (§5.1): a panel node edit writes verbatim into the
    // trusted vault, so scan the incoming content for data-exfiltration
    // payloads before persisting. Exfiltration-only scope (not Strict) so a
    // user's own security-research notes aren't rejected. On threat: reject,
    // do not write.
    if let Err(e) = crate::builtin_tools::note_manage::scan_note_for_exfiltration(&params.content) {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, e.to_string());
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

    // P1 partition isolation: unlike update/delete, rename has no natural
    // idempotent-success shape for a missing source — `NoteIndexer::
    // rename_note` hits the filesystem `rename()` syscall directly, and its
    // failure message embeds the real on-disk path, which would leak both
    // the invisible partition's content AND its directory layout if reused
    // verbatim. So this handler gets its own existence check BEFORE calling
    // into the indexer at all, mirroring `graph.node_detail`'s "Note not
    // found" shape (the read sibling of this family): an invisible
    // partition never even reaches the existence lookup (no oracle from
    // timing either), and a note that genuinely doesn't exist under a
    // visible partition now gets this same clean message instead of a
    // leaked OS path — a latent hygiene bug this fix also closes, since
    // there is no other way to make the two cases byte-identical without
    // duplicating `rename_note`'s internal path reconstruction.
    //
    // The existence check itself uses `find_by_filename(title, ..)` — NOT
    // an exact `(category, title)` match — because `rename_note` resolves
    // the real category by title lookup internally and deliberately
    // tolerates a stale/wrong category in the caller's `node_id` (see the
    // success-path comment below); an exact-path check here would reject
    // that already-working case as a false "not found".
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!("Note not found: {}", params.node_id),
        );
    }
    match indexer.store().find_by_filename(title, agent_id).await {
        Ok(paths) if !paths.is_empty() => {}
        Ok(_) => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                format!("Note not found: {}", params.node_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
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

    // P1 partition isolation: `delete_note` is idempotent — deleting a note
    // that never existed already reports success without erroring (see
    // `NoteIndexer::delete_note`'s doc). The no-oracle-consistent denial for
    // an invisible partition is that SAME success shape, without ever
    // touching the store: an unauthorized delete attempt looks identical to
    // deleting a note that was never there.
    if !crate::gateway::visibility::partition_visible(agent_id) {
        return JsonRpcResponse::success(
            req.id,
            serde_json::json!({ "node_id": params.node_id, "deleted": true }),
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

    /// P1's own acceptance case for `update_note`: bob "updating" alice's
    /// note reports the SAME success shape a legitimate write always
    /// produces (this handler's response is a fixed `{node_id, saved:true}`
    /// literal regardless of new-vs-existing — no dynamic content to
    /// byte-compare against), but nothing is actually written — alice's
    /// content on disk is untouched.
    #[tokio::test]
    async fn foreign_partition_update_reports_success_but_does_not_write() {
        use crate::gateway::caller_identity::CALLER_USER;

        let memory_dir = std::env::temp_dir().join(format!("update_deny_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db));

        let original = "---\ncategory: reference\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- alice's real fact\n";
        indexer
            .write_note_raw("main__u-alice", "reference", "AliceSecret", original)
            .await
            .unwrap();

        let req = update_note_request(
            "reference/AliceSecret",
            "bob's malicious overwrite",
            Some("main__u-alice"),
        );
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_update_note_impl(req, indexer).await
            })
            .await;
        assert!(resp.is_success(), "{:?}", resp.error);
        assert_eq!(resp.result.unwrap()["saved"], true);

        let path = memory_dir
            .join("main__u-alice")
            .join("reference")
            .join("AliceSecret.md");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            content, original,
            "denied update must leave alice's content untouched"
        );
    }

    /// P1's own acceptance case for `delete_note`: bob "deleting" alice's
    /// note reports the SAME idempotent-success shape a delete of a
    /// genuinely nonexistent note always produces, but the file and index
    /// entry are left completely intact — alice can still delete it herself
    /// afterward.
    #[tokio::test]
    async fn foreign_partition_delete_reports_success_but_leaves_the_note_intact() {
        use crate::gateway::caller_identity::CALLER_USER;

        let memory_dir = std::env::temp_dir().join(format!("delete_deny_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));

        indexer
            .write_note_raw(
                "main__u-alice",
                "plan",
                "AliceSecret",
                "---\ncategory: plan\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- x\n",
            )
            .await
            .unwrap();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.delete_note".into(),
            params: Some(serde_json::json!({
                "node_id": "plan/AliceSecret", "agent_id": "main__u-alice"
            })),
            id: Some(serde_json::json!(1)),
        };
        let resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_delete_note_impl(req, indexer.clone()).await
            })
            .await;
        assert!(resp.is_success(), "{:?}", resp.error);
        assert_eq!(resp.result.unwrap()["deleted"], true);

        assert!(
            memory_dir
                .join("main__u-alice")
                .join("plan/AliceSecret.md")
                .exists(),
            "denied delete must leave the file intact"
        );
        assert!(
            db.get_note_index("plan/AliceSecret", "main__u-alice")
                .await
                .unwrap()
                .is_some(),
            "denied delete must leave the index entry intact"
        );

        // Alice (the real owner) can still delete it for real.
        let alice_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.delete_note".into(),
            params: Some(serde_json::json!({
                "node_id": "plan/AliceSecret", "agent_id": "main__u-alice"
            })),
            id: Some(serde_json::json!(1)),
        };
        let alice_resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_delete_note_impl(alice_req, indexer).await
            })
            .await;
        assert!(alice_resp.is_success(), "{:?}", alice_resp.error);
        assert!(!memory_dir
            .join("main__u-alice")
            .join("plan/AliceSecret.md")
            .exists());
    }

    /// P1's own acceptance case for `rename_note`: bob renaming alice's note
    /// gets the exact same "Note not found" response a genuinely nonexistent
    /// node_id produces — compared against the SAME node_id on a fresh,
    /// never-seeded store so the comparison isolates the denial itself, not
    /// the node_id appearing in the message (the exact mistake caught and
    /// fixed in fix round 1). Alice's file is untouched under its old name,
    /// and she can still rename it herself afterward.
    #[tokio::test]
    async fn foreign_partition_rename_is_denied_with_the_not_found_shape_old_name_intact() {
        use crate::gateway::caller_identity::CALLER_USER;

        let memory_dir = std::env::temp_dir().join(format!("rename_deny_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db));

        indexer
            .write_note_raw(
                "main__u-alice",
                "reference",
                "AliceSecret",
                "---\ncategory: reference\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- x\n",
            )
            .await
            .unwrap();

        let rename_req = || JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.rename_note".into(),
            params: Some(serde_json::json!({
                "node_id": "reference/AliceSecret", "new_title": "Stolen", "agent_id": "main__u-alice"
            })),
            id: Some(serde_json::json!(1)),
        };

        let deny_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_rename_note_impl(rename_req(), indexer.clone()).await
            })
            .await;
        let deny_err = deny_resp.error.expect("must be denied");

        // SAME (node_id, agent_id), compared against a FRESH store where it
        // genuinely never existed.
        let empty_memory_dir =
            std::env::temp_dir().join(format!("rename_deny_empty_{}", Uuid::new_v4()));
        let empty_indexer = Arc::new(NoteIndexer::new(empty_memory_dir, make_db()));
        let missing_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_rename_note_impl(rename_req(), empty_indexer).await
            })
            .await;
        let missing_err = missing_resp
            .error
            .expect("genuinely missing must error too");

        assert_eq!(
            deny_err.message, missing_err.message,
            "denied and genuinely-missing must be byte-identical (no oracle)"
        );
        assert_eq!(deny_err.code, missing_err.code);

        // Alice's file is untouched under its old name.
        assert!(memory_dir
            .join("main__u-alice")
            .join("reference/AliceSecret.md")
            .exists());
        assert!(!memory_dir
            .join("main__u-alice")
            .join("reference/Stolen.md")
            .exists());

        // Alice (the real owner) can still rename it herself.
        let alice_resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_rename_note_impl(rename_req(), indexer).await
            })
            .await;
        assert!(alice_resp.is_success(), "{:?}", alice_resp.error);
    }
}

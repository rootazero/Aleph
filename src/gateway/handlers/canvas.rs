//! `canvas.*` — the whiteboard canvas RPC family.
//!
//! Eight methods over one [`CanvasStore`]: `create` / `list` / `get` /
//! `apply` / `delete` / `asset.put` / `asset.get` / `selection.set`.
//!
//! ## Discipline (the three rules every handler here follows)
//!
//! 1. **Gate before everything.** Every addressed handler resolves the
//!    document and asks [`visibility::canvas_visible`] BEFORE any store verb
//!    runs — a stranger probing `canvas.apply` with a stale revision must get
//!    not-found, never a conflict (the revision would be an existence oracle).
//!    The refusal is byte-identical to a missing id
//!    ([`projects`](super::projects) `project_not_found` precedent): both go
//!    through [`gate_canvas`], which fails closed on a store error too.
//! 2. **One error throat.** Store failures reach the wire only through
//!    [`canvas_error::respond`] (three-way split + `REVISION_CONFLICT`); the
//!    source guard in `canvas_error.rs` pins it.
//! 3. **Responses are BUILT from `aleph_protocol::canvas` contract types**,
//!    never assembled from loose `json!` maps — oversend is a compile error,
//!    not a hoped-for assertion (§0 "解析只证超集"). The key-set-equality
//!    test below derives its expectation from the contract type itself.
//!
//! `canvas.delete` is the one owner-only verb: visibility admits a roster
//! member, the owner gate then rejects them with `PERMISSION_DENIED` (they
//! can SEE the canvas, so there is no existence to hide — the honest refusal
//! names itself). Unlike `projects::require_owner` there is no org-admin
//! override: these handlers deliberately carry no `SecurityStore` context,
//! and an admin who must reap a canvas can act at the store root; widening
//! the signature for that rare verb would thread a second Arc through every
//! call site for one arm.
//!
//! `canvas.get` mints a canvas-scoped capability
//! ([`crate::gateway::security::CanvasCapabilities`], 10-minute TTL) and
//! fills `asset_base` with `/canvas-asset/<cap>/<canvas_id>` — the byte
//! route the Panel resolves `<image href>` against
//! (`server::canvas_asset_route`). Minting happens strictly AFTER
//! [`gate_canvas`] admitted the caller: the capability is a portable copy of
//! that visibility verdict, valid for its TTL. `canvas.create` deliberately
//! leaves `asset_base: None` — a brand-new canvas has no assets, and the
//! Panel's open path goes through `canvas.get` anyway.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use aleph_protocol::canvas::{
    AssetGetParams, AssetGetResult, AssetPutParams, AssetPutResult, CanvasApplyParams,
    CanvasApplyResult, CanvasCreateParams, CanvasDoc, CanvasEnvelope, CanvasList, CanvasRef,
    SelectionSetParams,
};

use super::{canvas_error, parse_params};
use crate::canvas::{selection, CanvasError, CanvasStore};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, PERMISSION_DENIED};
use crate::gateway::visibility;

/// Serialize a contract-typed response value. A contract type that will not
/// serialize is our bug, so the failure routes through the throat as
/// `Internal` rather than panicking a handler.
fn success_of<T: serde::Serialize>(id: Option<Value>, context: &str, value: &T) -> JsonRpcResponse {
    match serde_json::to_value(value) {
        Ok(v) => JsonRpcResponse::success(id, v),
        Err(e) => canvas_error::respond(
            id,
            context,
            &CanvasError::Internal(format!("failed to serialize response: {e}")),
        ),
    }
}

/// The response an unreachable canvas produces — whether it does not exist,
/// is scoped to someone else, or the gate itself failed. One constructor so
/// the causes stay indistinguishable (no-oracle), and its message is exactly
/// what [`CanvasStore::get`] emits for a missing id.
fn canvas_not_found(id: Option<Value>, context: &str, canvas_id: &str) -> JsonRpcResponse {
    canvas_error::respond(
        id,
        context,
        &CanvasError::NotFound(format!("canvas {canvas_id}")),
    )
}

/// The single admission point for every addressed `canvas.*` handler.
///
/// Resolves the document and checks [`visibility::canvas_visible`] (owner OR
/// roster member — the same predicate the event face and the tool face
/// resolve, §0 "一个动词有 N 个面"). Fails closed: a caller is never
/// admitted because the check itself could not complete. An unparseable
/// document also reads as not-found here — the store-level `get` keeps the
/// honest `Internal` for in-process callers, but the RPC face cannot check
/// visibility on a document it cannot read, and admitting would be the wrong
/// direction; the named `warn!` is what keeps the skip loud (§5.23b).
async fn gate_canvas(
    store: &CanvasStore,
    id: Option<Value>,
    context: &str,
    canvas_id: &str,
) -> Result<CanvasDoc, JsonRpcResponse> {
    match store.get(canvas_id).await {
        Ok(doc)
            if visibility::canvas_visible(
                doc.owner_user_id.as_deref(),
                doc.project_id.as_deref(),
            ) =>
        {
            Ok(doc)
        }
        Ok(_) => Err(canvas_not_found(id, context, canvas_id)),
        // A malformed id names no document anywhere, so telling the caller
        // how ids look leaks nothing and is actionable.
        Err(e @ CanvasError::Invalid(_)) => Err(canvas_error::respond(id, context, &e)),
        Err(CanvasError::NotFound(_)) => Err(canvas_not_found(id, context, canvas_id)),
        Err(e) => {
            tracing::warn!(canvas = %canvas_id, error = %e, "canvas: gate failed closed");
            Err(canvas_not_found(id, context, canvas_id))
        }
    }
}

/// `canvas.create { title?, project_id? }` — mint a canvas owned by the
/// caller. Creation has no addressed record to gate; the new row is stamped
/// with [`crate::scope::ambient_owner`] instead (`projects.create`
/// precedent). A `project_id` link WIDENS the audience to that room's roster,
/// so the writer passes the same membership test the readers will
/// (§5.22 "写路径合成了分区，读路径必须用同一个函数合成"): a room the
/// caller cannot see refuses exactly like a room that does not exist.
pub async fn handle_create(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to create canvas";
    let params: CanvasCreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(project_id) = params.project_id.as_deref() {
        if !visibility::project_visible(project_id) {
            return canvas_error::respond(
                request.id,
                CONTEXT,
                &CanvasError::NotFound(format!("project {project_id}")),
            );
        }
    }
    let owner = crate::scope::ambient_owner();
    match store.create(params.title, params.project_id, owner).await {
        Ok(doc) => {
            let selection = selection::get(&doc.id);
            success_of(
                request.id,
                CONTEXT,
                &CanvasEnvelope {
                    canvas: doc,
                    selection,
                    // Deliberately unminted: a brand-new canvas has no
                    // assets, and opening it goes through `canvas.get`,
                    // which mints the capability (see the module doc).
                    asset_base: None,
                },
            )
        }
        Err(e) => canvas_error::respond(request.id, CONTEXT, &e),
    }
}

/// `canvas.list` — the caller's visible slice of the library. Filtered
/// through the same predicate the addressed gate uses, over
/// [`CanvasStore::list_entries`] (the wire row deliberately omits the owner,
/// so the listing form carries it for exactly this filter).
pub async fn handle_list(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to list canvases";
    let canvases = store
        .list_entries()
        .await
        .into_iter()
        .filter(|entry| {
            visibility::canvas_visible(
                entry.owner_user_id.as_deref(),
                entry.row.project_id.as_deref(),
            )
        })
        .map(|entry| entry.row)
        .collect();
    success_of(request.id, CONTEXT, &CanvasList { canvases })
}

/// `canvas.get { canvas_id }` — the whole document plus the live selection.
pub async fn handle_get(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to get canvas";
    let params: CanvasRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc = match gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        Ok(doc) => doc,
        Err(e) => return e,
    };
    let selection = selection::get(&doc.id);
    // Minted only past the gate: the capability carries this caller's
    // visibility verdict to the `/canvas-asset/...` byte route for its TTL.
    // Fresh mints are idempotent, so the URL is stable across repeated gets
    // and the browser's image cache keeps working.
    let cap = crate::gateway::security::CanvasCapabilities::mint(&doc.id);
    let asset_base = Some(format!("/canvas-asset/{cap}/{}", doc.id));
    success_of(
        request.id,
        CONTEXT,
        &CanvasEnvelope {
            canvas: doc,
            selection,
            asset_base,
        },
    )
}

/// `canvas.apply { canvas_id, base_revision, ops }` — the only write entry
/// point. Gate first (a stale-revision probe from a stranger must answer
/// not-found, never `REVISION_CONFLICT`), then delegate; the store re-checks
/// the revision under the per-canvas lock, so the gate read racing another
/// writer costs nothing but a conflict the caller replays. Ops cannot change
/// the project link (`SetDocMeta` carries only `title`), so visibility
/// cannot be widened mid-flight by the very batch being admitted.
pub async fn handle_apply(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to apply canvas ops";
    let params: CanvasApplyParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        return e;
    }
    let actor = visibility::visible_owner_filter();
    match store
        .apply(&params.canvas_id, params.base_revision, params.ops, actor)
        .await
    {
        Ok(revision) => success_of(request.id, CONTEXT, &CanvasApplyResult { revision }),
        Err(e) => canvas_error::respond(request.id, CONTEXT, &e),
    }
}

/// `canvas.delete { canvas_id }` — the owner-only verb. Visibility admits a
/// roster member; the owner gate then refuses them by name (they can already
/// see the canvas, so `PERMISSION_DENIED` leaks no existence). An
/// unrestricted caller (`None`) passes, the same first arm every scoped
/// predicate opens with.
pub async fn handle_delete(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to delete canvas";
    let params: CanvasRef = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc = match gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        Ok(doc) => doc,
        Err(e) => return e,
    };
    if let Some(caller) = visibility::visible_owner_filter() {
        if caller != visibility::owner_or_legacy(doc.owner_user_id.as_deref()) {
            return JsonRpcResponse::error(request.id, PERMISSION_DENIED, "not the canvas owner");
        }
    }
    match store.delete(&params.canvas_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({})),
        Err(e) => canvas_error::respond(request.id, CONTEXT, &e),
    }
}

/// `canvas.asset.put { canvas_id, mime_type, data }` — store a
/// content-addressed asset (base64 in, `<sha256>.<ext>` out). Byte caps and
/// the mime whitelist live in the store; a payload that is not base64 at all
/// is rejected here as caller-fixable before any bytes are buffered further.
pub async fn handle_asset_put(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to store canvas asset";
    let params: AssetPutParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        return e;
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&params.data) {
        Ok(bytes) => bytes,
        Err(e) => {
            return canvas_error::respond(
                request.id,
                CONTEXT,
                &CanvasError::Invalid(format!("asset data is not valid base64: {e}")),
            )
        }
    };
    match store
        .put_asset(&params.canvas_id, &params.mime_type, &bytes)
        .await
    {
        Ok(asset_id) => success_of(request.id, CONTEXT, &AssetPutResult { asset_id }),
        Err(e) => canvas_error::respond(request.id, CONTEXT, &e),
    }
}

/// `canvas.asset.get { canvas_id, asset_id }` — read an asset back (base64).
pub async fn handle_asset_get(request: JsonRpcRequest, store: Arc<CanvasStore>) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to read canvas asset";
    let params: AssetGetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        return e;
    }
    match store.read_asset(&params.canvas_id, &params.asset_id).await {
        Ok((mime_type, bytes)) => success_of(
            request.id,
            CONTEXT,
            &AssetGetResult {
                mime_type,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        ),
        Err(e) => canvas_error::respond(request.id, CONTEXT, &e),
    }
}

/// `canvas.selection.set { canvas_id, shape_ids }` — record the caller's
/// live selection so the model can read it back through `canvas.get`. The
/// table is storage, not an authority (its module doc says so): the gate
/// here is what keeps a stranger from planting ids on a foreign canvas.
pub async fn handle_selection_set(
    request: JsonRpcRequest,
    store: Arc<CanvasStore>,
) -> JsonRpcResponse {
    const CONTEXT: &str = "Failed to set canvas selection";
    let params: SelectionSetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(e) = gate_canvas(&store, request.id.clone(), CONTEXT, &params.canvas_id).await {
        return e;
    }
    selection::set(&params.canvas_id, params.shape_ids);
    JsonRpcResponse::success(request.id, json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::lane::Lane;
    use crate::gateway::protocol::{INVALID_PARAMS, RESOURCE_NOT_FOUND, REVISION_CONFLICT};
    use crate::projects::roster::{publish, RosterSnapshot, TEST_GUARD as ROSTER_TEST_GUARD};
    use aleph_protocol::canvas::{CanvasOp, FracIndex, Shape, ShapeCommon, ShapeStyle};
    use serde_json::json;

    /// The tempdir guard is returned so it outlives every assertion — a
    /// dropped guard deletes the tree under a live store (§0 scratch rule).
    fn store() -> (tempfile::TempDir, Arc<CanvasStore>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()));
        (dir, store)
    }

    fn rpc(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    async fn as_user<F: std::future::Future<Output = JsonRpcResponse>>(
        user: &str,
        fut: F,
    ) -> JsonRpcResponse {
        CALLER_USER.scope(Some(user.to_string()), fut).await
    }

    fn err_of(resp: &JsonRpcResponse) -> (i32, String) {
        let e = resp.error.as_ref().expect("expected an error response");
        (e.code, e.message.clone())
    }

    fn note_op(id: &str) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: Shape::Note {
                common: ShapeCommon {
                    id: id.to_string(),
                    x: 0.0,
                    y: 0.0,
                    w: 200.0,
                    h: 200.0,
                    z: FracIndex::first(),
                    parent_id: None,
                },
                style: ShapeStyle::default(),
                text: "hi".to_string(),
            },
        }
    }

    /// Create a canvas as `owner` (optionally project-linked); returns its id.
    async fn create_as(store: &Arc<CanvasStore>, owner: &str, project_id: Option<&str>) -> String {
        let params = json!({ "title": "t", "project_id": project_id });
        let resp = as_user(
            owner,
            handle_create(rpc("canvas.create", params), store.clone()),
        )
        .await;
        resp.result.expect("create succeeds")["canvas"]["id"]
            .as_str()
            .expect("canvas id")
            .to_string()
    }

    fn apply_params(canvas_id: &str, base_revision: u64, ops: Vec<CanvasOp>) -> serde_json::Value {
        serde_json::to_value(CanvasApplyParams {
            canvas_id: canvas_id.to_string(),
            base_revision,
            ops,
        })
        .expect("params serialize")
    }

    /// The no-oracle contract: a canvas scoped to somebody else must be
    /// indistinguishable from an id that was never minted.
    #[tokio::test]
    async fn get_is_refused_for_a_stranger_as_not_found() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        let foreign = as_user(
            "u-mallory",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await;
        let missing = as_user(
            "u-mallory",
            handle_get(
                rpc("canvas.get", json!({ "canvas_id": "cv-does-not-exist" })),
                store.clone(),
            ),
        )
        .await;

        let (fc, fm) = err_of(&foreign);
        let (mc, mm) = err_of(&missing);
        assert_eq!(fc, RESOURCE_NOT_FOUND);
        assert_eq!(fc, mc, "same code");
        // Byte-identical modulo the id each one echoes, which the caller
        // supplied and already knows.
        assert_eq!(fm.replace(&id, "X"), mm.replace("cv-does-not-exist", "X"));
        assert!(foreign.result.is_none() && missing.result.is_none());
    }

    #[tokio::test]
    async fn a_roster_member_reads_a_project_linked_canvas_and_a_stranger_does_not() {
        let _guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        publish(RosterSnapshot::from_pairs([
            ("p-cv-room".to_string(), "u-alice".to_string()),
            ("p-cv-room".to_string(), "u-bob".to_string()),
        ]));
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", Some("p-cv-room")).await;

        let member = as_user(
            "u-bob",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await;
        assert!(member.result.is_some(), "roster member reads the canvas");

        let stranger = as_user(
            "u-mallory",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await;
        assert_eq!(err_of(&stranger).0, RESOURCE_NOT_FOUND);

        // The member also sees it in the library; the stranger's list is empty.
        let listed = as_user(
            "u-bob",
            handle_list(rpc("canvas.list", json!({})), store.clone()),
        )
        .await
        .result
        .expect("list");
        assert_eq!(listed["canvases"].as_array().expect("rows").len(), 1);
        let empty = as_user(
            "u-mallory",
            handle_list(rpc("canvas.list", json!({})), store.clone()),
        )
        .await
        .result
        .expect("list");
        assert!(empty["canvases"].as_array().expect("rows").is_empty());
    }

    /// Linking a canvas into a room the caller cannot see refuses exactly
    /// like a room that does not exist — the writer of the audience-widening
    /// field passes the same membership test its readers will.
    #[tokio::test]
    async fn create_with_a_foreign_project_link_is_refused_as_not_found() {
        let _guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        publish(RosterSnapshot::from_pairs([(
            "p-cv-private".to_string(),
            "u-alice".to_string(),
        )]));
        let (_dir, store) = store();

        let refused = as_user(
            "u-mallory",
            handle_create(
                rpc("canvas.create", json!({ "project_id": "p-cv-private" })),
                store.clone(),
            ),
        )
        .await;
        let (code, msg) = err_of(&refused);
        assert_eq!(code, RESOURCE_NOT_FOUND);
        assert!(msg.contains("p-cv-private"), "{msg}");
    }

    #[tokio::test]
    async fn apply_conflict_maps_to_revision_conflict_code() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        let ok = as_user(
            "u-alice",
            handle_apply(
                rpc("canvas.apply", apply_params(&id, 1, vec![note_op("n1")])),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(ok.result.expect("first apply lands")["revision"], json!(2));

        let stale = as_user(
            "u-alice",
            handle_apply(
                rpc("canvas.apply", apply_params(&id, 1, vec![note_op("n2")])),
                store.clone(),
            ),
        )
        .await;
        let (code, msg) = err_of(&stale);
        assert_eq!(code, REVISION_CONFLICT);
        assert!(
            msg.contains('2'),
            "conflict message must name the current revision: {msg}"
        );
    }

    /// Gate before everything: a stranger probing with a stale revision gets
    /// not-found, never `REVISION_CONFLICT` — the conflict (and the current
    /// revision in its message) would be an existence oracle.
    #[tokio::test]
    async fn apply_by_a_stranger_is_refused_as_not_found_never_conflict() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        let probe = as_user(
            "u-mallory",
            handle_apply(
                rpc("canvas.apply", apply_params(&id, 999, vec![note_op("n1")])),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&probe).0, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_by_a_roster_member_who_is_not_owner_is_refused() {
        let _guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        publish(RosterSnapshot::from_pairs([
            ("p-cv-del".to_string(), "u-alice".to_string()),
            ("p-cv-del".to_string(), "u-bob".to_string()),
        ]));
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", Some("p-cv-del")).await;

        // The member can see it, so the refusal names itself instead of
        // hiding behind not-found.
        let member = as_user(
            "u-bob",
            handle_delete(
                rpc("canvas.delete", json!({ "canvas_id": id })),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&member).0, PERMISSION_DENIED);

        // A stranger's delete probe reads exactly like a missing id.
        let stranger = as_user(
            "u-mallory",
            handle_delete(
                rpc("canvas.delete", json!({ "canvas_id": id })),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&stranger).0, RESOURCE_NOT_FOUND);

        // The owner's delete lands.
        let owner = as_user(
            "u-alice",
            handle_delete(
                rpc("canvas.delete", json!({ "canvas_id": id })),
                store.clone(),
            ),
        )
        .await;
        assert!(owner.result.is_some(), "owner deletes their canvas");
        let gone = as_user(
            "u-alice",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await;
        assert_eq!(err_of(&gone).0, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn asset_put_and_get_round_trip_base64() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;
        let bytes: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3];
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);

        let put = as_user(
            "u-alice",
            handle_asset_put(
                rpc(
                    "canvas.asset.put",
                    json!({ "canvas_id": id, "mime_type": "image/png", "data": data }),
                ),
                store.clone(),
            ),
        )
        .await;
        let asset_id = put.result.expect("put succeeds")["asset_id"]
            .as_str()
            .expect("asset id")
            .to_string();

        let got = as_user(
            "u-alice",
            handle_asset_get(
                rpc(
                    "canvas.asset.get",
                    json!({ "canvas_id": id, "asset_id": asset_id }),
                ),
                store.clone(),
            ),
        )
        .await;
        let got = got.result.expect("get succeeds");
        assert_eq!(got["mime_type"], json!("image/png"));
        assert_eq!(got["data"], json!(data));

        // A stranger's asset read refuses as not-found at the canvas gate.
        let stranger = as_user(
            "u-mallory",
            handle_asset_get(
                rpc(
                    "canvas.asset.get",
                    json!({ "canvas_id": id, "asset_id": asset_id }),
                ),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&stranger).0, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn asset_put_rejects_bad_base64_as_invalid_params() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;
        let bad = as_user(
            "u-alice",
            handle_asset_put(
                rpc(
                    "canvas.asset.put",
                    json!({ "canvas_id": id, "mime_type": "image/png", "data": "%%not-base64%%" }),
                ),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&bad).0, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn selection_set_shows_up_in_the_get_envelope() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        let set = as_user(
            "u-alice",
            handle_selection_set(
                rpc(
                    "canvas.selection.set",
                    json!({ "canvas_id": id, "shape_ids": ["s1", "s2"] }),
                ),
                store.clone(),
            ),
        )
        .await;
        assert!(set.result.is_some());

        let got = as_user(
            "u-alice",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await;
        assert_eq!(got.result.expect("get")["selection"], json!(["s1", "s2"]));

        // A stranger cannot plant a selection on a foreign canvas.
        let stranger = as_user(
            "u-mallory",
            handle_selection_set(
                rpc(
                    "canvas.selection.set",
                    json!({ "canvas_id": id, "shape_ids": ["x"] }),
                ),
                store.clone(),
            ),
        )
        .await;
        assert_eq!(err_of(&stranger).0, RESOURCE_NOT_FOUND);
    }

    /// Key-set equality, both directions, expectation derived from the
    /// contract type itself — parsing alone proves a superset (serde ignores
    /// unknown keys), and a literal key list is the same enumeration mistake
    /// one level up (`workspace.rs::
    /// the_workspace_responses_carry_the_contract_and_nothing_else` twin).
    #[tokio::test]
    async fn the_canvas_responses_carry_the_contract_and_nothing_else() {
        use std::collections::BTreeSet;

        fn keys_of<T: serde::Serialize>(v: &T) -> BTreeSet<String> {
            serde_json::to_value(v)
                .expect("contract types serialize")
                .as_object()
                .expect("contract projections are objects")
                .keys()
                .cloned()
                .collect()
        }
        fn emitted_keys(v: &serde_json::Value) -> BTreeSet<String> {
            v.as_object()
                .expect("response is an object")
                .keys()
                .cloned()
                .collect()
        }

        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        // --- get: the envelope, exactly --------------------------------
        let got = as_user(
            "u-alice",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await
        .result
        .expect("get");
        let parsed: CanvasEnvelope =
            serde_json::from_value(got.clone()).expect("the contract must parse it");
        assert_eq!(
            emitted_keys(&got),
            keys_of(&parsed),
            "canvas.get must emit the CanvasEnvelope key set and nothing else"
        );
        assert_eq!(
            emitted_keys(&got["canvas"]),
            keys_of(&parsed.canvas),
            "the embedded document must emit the CanvasDoc key set and nothing else"
        );

        // --- list: top level and the row, exactly ----------------------
        let listed = as_user(
            "u-alice",
            handle_list(rpc("canvas.list", json!({})), store.clone()),
        )
        .await
        .result
        .expect("list");
        let parsed: CanvasList =
            serde_json::from_value(listed.clone()).expect("the contract must parse it");
        assert_eq!(emitted_keys(&listed), keys_of(&parsed));
        assert_eq!(
            emitted_keys(&listed["canvases"][0]),
            keys_of(&parsed.canvases[0]),
            "canvas.list rows must emit the CanvasRow key set and nothing else"
        );

        // --- apply: the result, exactly --------------------------------
        let applied = as_user(
            "u-alice",
            handle_apply(
                rpc("canvas.apply", apply_params(&id, 1, vec![note_op("n1")])),
                store.clone(),
            ),
        )
        .await
        .result
        .expect("apply");
        let parsed: CanvasApplyResult =
            serde_json::from_value(applied.clone()).expect("the contract must parse it");
        assert_eq!(
            emitted_keys(&applied),
            keys_of(&parsed),
            "canvas.apply must emit the CanvasApplyResult key set and nothing else"
        );
    }

    /// `canvas.get` mints `asset_base` = `/canvas-asset/<cap>/<id>` where
    /// `<cap>` is the live canvas capability — bound to THIS canvas and to
    /// no other (the byte route resolves the canvas from the capability and
    /// requires the URL to agree). Stability across gets is what keeps the
    /// browser's image cache warm.
    #[tokio::test]
    async fn get_mints_an_asset_base_bound_to_the_canvas() {
        let (_dir, store) = store();
        let id = create_as(&store, "u-alice", None).await;

        let envelope = as_user(
            "u-alice",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await
        .result
        .expect("get");
        let parsed: CanvasEnvelope =
            serde_json::from_value(envelope).expect("the contract must parse it");
        let base = parsed.asset_base.expect("get must mint an asset base");

        let expected_suffix = format!("/{id}");
        assert!(
            base.starts_with("/canvas-asset/") && base.ends_with(&expected_suffix),
            "asset_base must be /canvas-asset/<cap>/<canvas_id>, got {base}"
        );
        let cap = base
            .trim_start_matches("/canvas-asset/")
            .trim_end_matches(&expected_suffix);
        assert!(
            crate::gateway::security::CanvasCapabilities::validate(cap, &id),
            "the embedded capability must validate for this canvas"
        );
        assert!(
            !crate::gateway::security::CanvasCapabilities::validate(cap, "cv-other"),
            "…and for no other canvas"
        );

        // Idempotent while fresh: a second get hands back the same URL.
        let again = as_user(
            "u-alice",
            handle_get(rpc("canvas.get", json!({ "canvas_id": id })), store.clone()),
        )
        .await
        .result
        .expect("get");
        let again: CanvasEnvelope = serde_json::from_value(again).expect("parses");
        assert_eq!(again.asset_base.as_deref(), Some(base.as_str()));
    }

    /// Lane pin: the three read methods land in Query on the suffix
    /// heuristic alone (`lane.rs` deliberately has no canvas entries), and a
    /// rename that broke this would strand them in Mutate behind the
    /// idempotency-key guard (§6.8). The write methods stay in Mutate.
    #[test]
    fn every_canvas_read_lands_in_query_lane() {
        for m in ["canvas.get", "canvas.list", "canvas.asset.get"] {
            assert_eq!(Lane::for_method(m), Lane::Query, "{m}");
        }
        for m in [
            "canvas.create",
            "canvas.apply",
            "canvas.delete",
            "canvas.asset.put",
            "canvas.selection.set",
        ] {
            assert_eq!(Lane::for_method(m), Lane::Mutate, "{m}");
        }
    }
}

//! `canvas.*` RPC surface — the whiteboard behind `/canvas`.
//!
//! # Typed against the shared contract, not against hand-written structs
//!
//! Every shape here is [`aleph_protocol::canvas`], the same module the server
//! **builds** its responses from (`src/gateway/handlers/canvas.rs`). Same
//! reasoning as [`super::workspace`]: a shared type makes a rename a compile
//! error on every client at once. No `json!({ … })` literals below and no
//! local mirror structs — a missing field goes into the protocol crate.
//!
//! # How a revision conflict reaches this crate
//!
//! The server refuses a stale `canvas.apply` with its own JSON-RPC code —
//! [`aleph_protocol::jsonrpc::REVISION_CONFLICT`], the same constant its one
//! mapping site (`gateway::handlers::canvas_error::respond`) emits, so a
//! renumber is a compile error on both sides at once. The code reaches this
//! crate through `rpc_call_with_code` (the message loop keeps the full error
//! object since 2026-08-17; the flat `rpc_call` face projects it down to
//! `message` for every legacy consumer), and [`CanvasApi::apply`] classifies
//! it **here, at the API boundary**, into [`CanvasApplyError`]: the editor
//! branches on the enum, never on message text. The conflict message still
//! names the current revision (`the_conflict_message_names_the_current_revision`
//! pins it server-side) — but that wording is for humans and models now, not
//! a protocol surface.
//!
//! # One producer for the `canvas.*` wire strings
//!
//! Every `canvas.*` RPC in this crate is issued from this file — pinned by
//! the source guard below. The event **topic** is not an RPC and not a
//! string here either: consumers subscribe via
//! [`aleph_protocol::canvas::TOPIC`], so the method family has exactly one
//! spelling site per side of the wire.

use aleph_protocol::canvas::{
    AssetGetParams, AssetGetResult, AssetPutParams, AssetPutResult, CanvasApplyParams,
    CanvasApplyResult, CanvasCreateParams, CanvasDoc, CanvasEnvelope, CanvasList, CanvasOp,
    CanvasRef, CanvasRow, SelectionSetParams,
};
use serde_json::Value;

use crate::context::{DashboardState, RpcFailure};

/// Why a `canvas.apply` failed, classified at the API boundary.
///
/// The one consumer that must tell these apart is the editor's send loop:
/// a [`Conflict`](Self::Conflict) triggers refetch-and-replay, anything else
/// drops the pending batch and refetches server truth. Classification lives
/// here — where the wire is parsed — so the editor never sees a raw failure
/// it would be tempted to string-match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasApplyError {
    /// The server refused the batch because `base_revision` was stale
    /// (`REVISION_CONFLICT` on the wire). Someone else landed first;
    /// recovery — not surfacing — is the correct response.
    Conflict,
    /// Every other refusal or transport failure, message verbatim.
    Other(String),
}

impl From<RpcFailure> for CanvasApplyError {
    fn from(failure: RpcFailure) -> Self {
        if failure.code == Some(aleph_protocol::jsonrpc::REVISION_CONFLICT) {
            Self::Conflict
        } else {
            Self::Other(failure.message)
        }
    }
}

/// Whiteboard canvas calls.
pub struct CanvasApi;

/// Serialize params, tagging the method so a failure names itself.
///
/// These cannot fail for the contract types as they stand, but the
/// alternative is `.unwrap()` in a WASM binary, where a panic takes the whole
/// Panel down rather than one page.
fn encode<T: serde::Serialize>(method: &str, params: &T) -> Result<Value, String> {
    serde_json::to_value(params).map_err(|e| format!("Failed to encode {method} params: {e}"))
}

/// Parse a [`CanvasEnvelope`] — `create` and `get` both answer in it.
///
/// One helper rather than two call sites, because the failure it reports is
/// "the server sent a shape this Panel cannot read", and that sentence should
/// not have two wordings.
fn envelope(method: &str, result: Value) -> Result<CanvasEnvelope, String> {
    serde_json::from_value::<CanvasEnvelope>(result)
        .map_err(|e| format!("Invalid {method} response: {e}"))
}

impl CanvasApi {
    /// List the caller's visible slice of the canvas library.
    ///
    /// A missing `canvases` key is an error rather than an empty library —
    /// [`CanvasList`] has no serde default for exactly this reason: "you have
    /// no canvases" and "this response is broken" render identically
    /// otherwise.
    pub async fn list(state: &DashboardState) -> Result<Vec<CanvasRow>, String> {
        let result = state.rpc_call("canvas.list", serde_json::json!({})).await?;
        serde_json::from_value::<CanvasList>(result)
            .map(|list| list.canvases)
            .map_err(|e| format!("Invalid canvas.list response: {e}"))
    }

    /// Create a canvas and return the new document.
    ///
    /// The server answers with a full [`CanvasEnvelope`] (empty selection,
    /// `asset_base` deliberately unminted — a brand-new canvas has no assets
    /// and opening it goes through [`Self::get`], which mints one); the
    /// document is the part a caller can use.
    pub async fn create(
        state: &DashboardState,
        title: Option<String>,
        project_id: Option<String>,
    ) -> Result<CanvasDoc, String> {
        let params = encode("canvas.create", &CanvasCreateParams { title, project_id })?;
        let result = state.rpc_call("canvas.create", params).await?;
        envelope("canvas.create", result).map(|e| e.canvas)
    }

    /// Fetch one canvas: the whole document, the live selection, and a
    /// freshly minted capability `asset_base`.
    pub async fn get(state: &DashboardState, id: &str) -> Result<CanvasEnvelope, String> {
        let params = encode(
            "canvas.get",
            &CanvasRef {
                canvas_id: id.to_string(),
            },
        )?;
        let result = state.rpc_call("canvas.get", params).await?;
        envelope("canvas.get", result)
    }

    /// Apply an ops batch against `base_revision`; returns the new revision.
    ///
    /// On a stale base the `Err` is [`CanvasApplyError::Conflict`], classified
    /// off the wire's `REVISION_CONFLICT` code (see the module doc); every
    /// other failure carries its message verbatim in
    /// [`CanvasApplyError::Other`].
    pub async fn apply(
        state: &DashboardState,
        id: &str,
        base_revision: u64,
        ops: Vec<CanvasOp>,
    ) -> Result<u64, CanvasApplyError> {
        let params = encode(
            "canvas.apply",
            &CanvasApplyParams {
                canvas_id: id.to_string(),
                base_revision,
                ops,
            },
        )
        .map_err(CanvasApplyError::Other)?;
        let result = state
            .rpc_call_with_code("canvas.apply", params)
            .await
            .map_err(CanvasApplyError::from)?;
        serde_json::from_value::<CanvasApplyResult>(result)
            .map(|r| r.revision)
            .map_err(|e| CanvasApplyError::Other(format!("Invalid canvas.apply response: {e}")))
    }

    /// Delete a canvas — the owner-only verb. The server answers `{}`;
    /// callers refetch the list.
    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = encode(
            "canvas.delete",
            &CanvasRef {
                canvas_id: id.to_string(),
            },
        )?;
        state.rpc_call("canvas.delete", params).await?;
        Ok(())
    }

    /// Store a content-addressed asset (base64 in, `<sha256>.<ext>` out).
    pub async fn asset_put(
        state: &DashboardState,
        id: &str,
        mime: &str,
        data_base64: String,
    ) -> Result<String, String> {
        let params = encode(
            "canvas.asset.put",
            &AssetPutParams {
                canvas_id: id.to_string(),
                mime_type: mime.to_string(),
                data: data_base64,
            },
        )?;
        let result = state.rpc_call("canvas.asset.put", params).await?;
        serde_json::from_value::<AssetPutResult>(result)
            .map(|r| r.asset_id)
            .map_err(|e| format!("Invalid canvas.asset.put response: {e}"))
    }

    /// Read an asset back as `(mime_type, base64)`.
    ///
    /// This is how the Panel obtains `text/html` asset *source*: the
    /// capability byte route deliberately serves html as `text/plain` (the
    /// XSS boundary — a capability URL opened directly must never become a
    /// same-origin HTML page), so iframe `srcdoc` content comes through the
    /// RPC instead. Images keep using the byte route (`asset_base`), which
    /// the browser can cache.
    pub async fn asset_get(
        state: &DashboardState,
        id: &str,
        asset_id: &str,
    ) -> Result<AssetGetResult, String> {
        let params = encode(
            "canvas.asset.get",
            &AssetGetParams {
                canvas_id: id.to_string(),
                asset_id: asset_id.to_string(),
            },
        )?;
        let result = state.rpc_call("canvas.asset.get", params).await?;
        serde_json::from_value::<AssetGetResult>(result)
            .map_err(|e| format!("Invalid canvas.asset.get response: {e}"))
    }

    /// Push this client's selection so the model can read it back through
    /// `canvas.get`. Last write wins across clients — by design.
    pub async fn selection_set(
        state: &DashboardState,
        id: &str,
        shape_ids: Vec<String>,
    ) -> Result<(), String> {
        let params = encode(
            "canvas.selection.set",
            &SelectionSetParams {
                canvas_id: id.to_string(),
                shape_ids,
            },
        )?;
        state.rpc_call("canvas.selection.set", params).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conflict classifier branches on the wire code — the same
    /// [`aleph_protocol::jsonrpc::REVISION_CONFLICT`] constant the server's
    /// one mapping site emits, so this assertion and the server's
    /// `apply_conflict_maps_to_revision_conflict_code` read one value.
    #[test]
    fn a_revision_conflict_is_classified_by_its_wire_code() {
        let failure = RpcFailure {
            code: Some(aleph_protocol::jsonrpc::REVISION_CONFLICT),
            message: "Failed to apply canvas ops: revision conflict: canvas is at revision 7"
                .to_string(),
        };
        assert_eq!(CanvasApplyError::from(failure), CanvasApplyError::Conflict);
    }

    /// Phrase-matching is dead: a failure whose *text* happens to contain the
    /// conflict wording but carries no code (a transport error quoting a log
    /// line, a proxy page) must NOT trigger refetch-and-replay. This is the
    /// negative half the old `is_revision_conflict(message)` detector could
    /// never have — it was retired when `rpc_call_with_code` landed.
    #[test]
    fn the_conflict_phrase_without_the_code_is_not_a_conflict() {
        let failure = RpcFailure {
            code: None,
            message: "upstream said: revision conflict: canvas is at revision 7".to_string(),
        };
        assert_eq!(
            CanvasApplyError::from(failure.clone()),
            CanvasApplyError::Other(failure.message)
        );

        let wrong_code = RpcFailure {
            code: Some(aleph_protocol::jsonrpc::INTERNAL_ERROR),
            message: "revision conflict".to_string(),
        };
        assert!(matches!(
            CanvasApplyError::from(wrong_code),
            CanvasApplyError::Other(_)
        ));
    }

    /// A missing `canvases` key is a broken response, not an empty library.
    #[test]
    fn a_response_missing_the_key_is_an_error_not_an_empty_library() {
        let good = serde_json::json!({
            "canvases": [{
                "id": "cv-1",
                "title": "Sketch",
                "revision": 3,
                "shape_count": 7,
                "updated_at_ms": 1_700_000_000_000_i64,
            }]
        });
        let parsed: CanvasList = serde_json::from_value(good).expect("a real response parses");
        assert_eq!(parsed.canvases.len(), 1);
        assert_eq!(parsed.canvases[0].id, "cv-1");

        assert!(
            serde_json::from_value::<CanvasList>(serde_json::json!({})).is_err(),
            "a missing `canvases` key must not read as an empty library"
        );
    }

    /// The envelope helper reports a shape it cannot read, and names the
    /// method that produced it.
    #[test]
    fn a_shape_this_panel_cannot_read_is_reported_as_such() {
        let err = envelope("canvas.get", serde_json::json!({ "nope": 1 }))
            .expect_err("a missing envelope key must not parse");
        assert!(err.contains("canvas.get"), "{err}");
    }

    /// Every `canvas.*` RPC string in this crate is spelled in this file and
    /// nowhere else.
    ///
    /// # Why source-level
    ///
    /// A second call site with its own `"canvas.apply"` literal compiles,
    /// works, and silently forks the wire surface the day this family gains a
    /// param or renames a method — the exact drift the shared contract types
    /// exist to prevent, re-entering through a string. At runtime a literal
    /// here and a literal there are indistinguishable.
    ///
    /// The event topic is covered by the same sweep for free: consumers use
    /// [`aleph_protocol::canvas::TOPIC`], never the string, so `"canvas.`
    /// simply does not appear outside this file. Comment lines are stripped
    /// first — a doc sentence mentioning `"canvas.get"` is documentation, not
    /// a call site (§0: scanners judge code, comments are prose).
    #[test]
    fn canvas_rpc_is_issued_from_api_canvas_alone() {
        let root = crate::disposed_reads::src_dir();
        let sources = crate::disposed_reads::rust_sources(&root);
        assert!(
            sources.len() > 50,
            "found almost no sources — the walk is broken, not the code"
        );

        let mut offenders = Vec::new();
        let mut this_file_seen = false;
        for path in sources {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let is_this_file = path.ends_with("api/canvas.rs");
            let code: String = src
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("//") && !t.starts_with("//!")
                })
                .collect::<Vec<_>>()
                .join("\n");
            if code.contains("\"canvas.") {
                if is_this_file {
                    this_file_seen = true;
                } else {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            this_file_seen,
            "api/canvas.rs no longer spells any canvas.* method — the positive \
             control failed, so this guard is scanning the wrong tree"
        );
        assert!(
            offenders.is_empty(),
            "canvas.* wire strings outside api/canvas.rs — route these through \
             CanvasApi (or aleph_protocol::canvas::TOPIC for the event topic):\n{}",
            offenders.join("\n")
        );
    }
}

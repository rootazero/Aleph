//! `canvas`: the model's face of the whiteboard (R8 twin of the `canvas.*`
//! RPC family).
//!
//! Redline: pure I/O translation (R4), no reasoning (R7). Every verdict —
//! visibility, revision conflicts, asset caps, the mime whitelist — is
//! [`crate::canvas::CanvasStore`]'s, the SAME instance the RPC handlers call.
//! What is here is argument shape, the three-form `location` resolver for
//! `insert_image`, and rendering.
//!
//! # Gating: the same predicate as the RPC face, resolved for a run
//!
//! Every addressed action gates on
//! [`crate::gateway::visibility::ambient_canvas_visible`] — the tool-face
//! resolver of the ONE predicate the RPC face (`canvas_visible`) and the
//! event face (`ByCanvasScope`) also resolve. `CALLER_USER` is dead inside a
//! spawned run, so wiring the RPC twin here would be constantly-true
//! (§5.22 round 2 ⑤). An invisible canvas answers byte-identically to a
//! missing one (no existence oracle).
//!
//! # Why `apply` takes no `base_revision`
//!
//! The optimistic-lock protocol is the Panel's problem to replay; the model
//! would only ever echo a number it read one breath earlier. The tool reads
//! the current revision itself, applies, and on a conflict retries ONCE
//! against the revision the conflict names — a second conflict surfaces as a
//! compact error the model can act on (A2: let the model see and self-heal,
//! never a retry matrix in the harness).
//!
//! # Why `delete` is not an action
//!
//! Whole-canvas deletion is the RPC face's owner-only verb. The tool face
//! deliberately does not carry it — `apply` with `delete_shape` covers every
//! editing need, and an irreversible whole-document verb on the model's side
//! of the table buys nothing but a confirmation-gate question.
//!
//! Not read-only (`READ_ONLY_TOOLS`): one name multiplexes reads and writes,
//! exactly like `file_ops` / `workspace_manage`, so declaring the whole tool
//! idempotent would tell the exec tier a write is a read.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use aleph_protocol::canvas::{
    AiFrameStatus, CanvasDoc, CanvasOp, FracIndex, Shape, ShapeCommon, MAX_ASSET_BYTES,
};

use crate::canvas::{selection, CanvasError, CanvasStore};
use crate::error::{AlephError, Result};
use crate::gateway::visibility;
use crate::tools::AlephTool;

/// Default placement box when neither a frame nor coordinates are given.
const DEFAULT_BOX: (f64, f64, f64, f64) = (100.0, 100.0, 512.0, 512.0);

/// Default 16:9 frame for `insert_html`.
const DEFAULT_HTML_FRAME: (f64, f64) = (960.0, 540.0);

/// Wall-clock budget for one `insert_image` https fetch.
const HTTP_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Excerpt cap for `get(detail="summary")`, in characters (`chars`, not
/// bytes — a CJK title must not get a third of the budget, §2.16).
const EXCERPT_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanvasToolAction {
    List,
    Create,
    Get,
    Apply,
    InsertImage,
    InsertHtml,
    ReadAsset,
}

impl CanvasToolAction {
    /// The wire spelling, for refusals that name the action they belong to.
    fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Create => "create",
            Self::Get => "get",
            Self::Apply => "apply",
            Self::InsertImage => "insert_image",
            Self::InsertHtml => "insert_html",
            Self::ReadAsset => "read_asset",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasToolArgs {
    pub action: CanvasToolAction,
    /// Canvas id (`cv-…`, from list/create) — required by every action
    /// except `list` and `create`.
    #[serde(default)]
    pub canvas_id: Option<String>,
    /// get: "summary" (default) or "full".
    #[serde(default)]
    pub detail: Option<String>,
    /// create: canvas title. insert_html without `frame_id`: the new frame's
    /// title.
    #[serde(default)]
    pub title: Option<String>,
    /// create: link the canvas to a project room (its roster can then see it).
    #[serde(default)]
    pub project_id: Option<String>,
    /// apply: the op batch.
    #[serde(default)]
    pub ops: Option<Vec<CanvasOp>>,
    /// insert_image: `data:` URL, local file path, or https URL.
    #[serde(default)]
    pub location: Option<String>,
    /// insert_html: the single-file HTML body.
    #[serde(default)]
    pub html: Option<String>,
    /// insert_*: target frame — the image replaces it in place; the html
    /// replaces the frame's html child.
    #[serde(default)]
    pub frame_id: Option<String>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub w: Option<f64>,
    #[serde(default)]
    pub h: Option<f64>,
    /// read_asset: asset id (`<sha256>.<ext>`, as referenced by shapes).
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Clone)]
pub struct CanvasTool {
    /// The SAME `Arc` the gateway handed the RPC handlers, injected from
    /// `BuiltinToolConfig::canvas_store`.
    ///
    /// Not a store this tool opens for itself, and the reason is not
    /// tidiness: `CanvasStore` publishes `canvas.updated` from inside its
    /// per-canvas critical section, and only the instance built at startup
    /// has the event bus attached (`CanvasStore::with_event_bus`). A
    /// hand-rolled `CanvasStore::new(get_canvas_root()?)` would work
    /// perfectly and silently stop every open Panel from refreshing —
    /// the `workspace_manage` lesson, verbatim.
    store: Arc<CanvasStore>,
}

impl CanvasTool {
    #[must_use]
    pub const fn new(store: Arc<CanvasStore>) -> Self {
        Self { store }
    }

    /// The canvas id an addressed action needs, or a refusal naming it.
    fn addressed(action: CanvasToolAction, args: &CanvasToolArgs) -> Result<String> {
        match args.canvas_id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => Ok(id.to_string()),
            _ => Err(AlephError::tool(format!(
                "{}: `canvas_id` is required (a `cv-…` id, as shown by action=\"list\")",
                action.name()
            ))),
        }
    }

    /// Resolve + visibility-gate one canvas for this run's ambient actor.
    ///
    /// Fails closed, and an invisible canvas is byte-identical to a missing
    /// one — both render the store's own `NotFound` display, so the refusal
    /// is not an existence oracle.
    async fn gate(&self, action: CanvasToolAction, canvas_id: &str) -> Result<CanvasDoc> {
        match self.store.get(canvas_id).await {
            Ok(doc)
                if visibility::ambient_canvas_visible(
                    doc.owner_user_id.as_deref(),
                    doc.project_id.as_deref(),
                ) =>
            {
                Ok(doc)
            }
            Ok(_) => Err(not_found(canvas_id)),
            // A malformed id names no document anywhere; telling the model
            // how ids look is actionable and leaks nothing.
            Err(CanvasError::Invalid(e)) => {
                Err(AlephError::tool(format!("{}: {e}", action.name())))
            }
            Err(CanvasError::NotFound(_)) => Err(not_found(canvas_id)),
            Err(e) => {
                tracing::warn!(canvas = %canvas_id, error = %e,
                    "canvas tool: gate failed closed");
                Err(not_found(canvas_id))
            }
        }
    }

    /// Apply `ops` from `base_revision`, retrying ONCE on a conflict against
    /// the revision the conflict names. Callers pass the revision of the doc
    /// they just gated; the conflict arm needs no re-read because
    /// `CanvasError::Conflict` already carries the current revision.
    async fn apply_from(
        &self,
        canvas_id: &str,
        base_revision: u64,
        ops: Vec<CanvasOp>,
        actor: Option<String>,
    ) -> Result<u64> {
        let first = self
            .store
            .apply(canvas_id, base_revision, ops.clone(), actor.clone())
            .await;
        let retried = match first {
            Err(CanvasError::Conflict { current_revision }) => {
                self.store
                    .apply(canvas_id, current_revision, ops, actor)
                    .await
            }
            other => other,
        };
        retried.map_err(|e| match e {
            // Compact self-heal instruction (A2): the display already names
            // the current revision; tell the model the way back.
            CanvasError::Conflict { .. } => AlephError::tool(format!(
                "apply: {e} after one retry — re-read with action=\"get\" and re-issue"
            )),
            other => AlephError::tool(format!("apply: {other}")),
        })
    }

    /// The z index that stacks a new shape above everything on `doc`.
    fn top_z(doc: &CanvasDoc) -> FracIndex {
        doc.shapes
            .iter()
            .map(|s| &s.common().z)
            .max()
            .map_or_else(FracIndex::first, |max| FracIndex::between(Some(max), None))
    }

    /// Where an inserted shape lands: the target frame's box (replace mode)
    /// or explicit/default coordinates.
    fn placement_of(
        args: &CanvasToolArgs,
        doc: &CanvasDoc,
        default_wh: (f64, f64),
    ) -> Result<Placement> {
        if let Some(frame_id) = args.frame_id.as_deref() {
            let Some(frame) = doc.shapes.iter().find(|s| s.id() == frame_id) else {
                return Err(AlephError::tool(format!(
                    "not found: shape {frame_id} on canvas {} (list shapes with action=\"get\")",
                    doc.id
                )));
            };
            let c = frame.common();
            return Ok(Placement {
                x: c.x,
                y: c.y,
                w: c.w,
                h: c.h,
                z: c.z.clone(),
                parent_id: c.parent_id.clone(),
                frame: Some(frame.clone()),
            });
        }
        let (dx, dy, dw, dh) = (DEFAULT_BOX.0, DEFAULT_BOX.1, default_wh.0, default_wh.1);
        Ok(Placement {
            x: args.x.unwrap_or(dx),
            y: args.y.unwrap_or(dy),
            w: args.w.unwrap_or(dw),
            h: args.h.unwrap_or(dh),
            z: Self::top_z(doc),
            parent_id: None,
            frame: None,
        })
    }

    async fn insert_image(&self, args: &CanvasToolArgs) -> Result<Value> {
        let canvas_id = Self::addressed(CanvasToolAction::InsertImage, args)?;
        let doc = self.gate(CanvasToolAction::InsertImage, &canvas_id).await?;
        let location = args
            .location
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty());
        let Some(location) = location else {
            return Err(AlephError::tool(
                "insert_image: `location` is required (a data: URL, a local file path, or an https URL)"
                    .to_string(),
            ));
        };
        let (mime, bytes) = resolve_image_location(location).await?;
        let asset_id = self
            .store
            .put_asset(&canvas_id, &mime, &bytes)
            .await
            .map_err(|e| AlephError::tool(format!("insert_image: {e}")))?;

        let place = Self::placement_of(args, &doc, (DEFAULT_BOX.2, DEFAULT_BOX.3))?;
        let shape_id = mint_shape_id();
        let mut ops = vec![CanvasOp::UpsertShape {
            shape: Shape::Image {
                common: ShapeCommon {
                    id: shape_id.clone(),
                    x: place.x,
                    y: place.y,
                    w: place.w,
                    h: place.h,
                    z: place.z.clone(),
                    parent_id: place.parent_id.clone(),
                },
                asset_id: asset_id.clone(),
                natural_w: 0.0,
                natural_h: 0.0,
            },
        }];
        let replaced_frame = place.frame.as_ref().map(|f| f.id().to_string());
        if let Some(frame_id) = &replaced_frame {
            ops.push(CanvasOp::DeleteShape {
                id: frame_id.clone(),
            });
        }
        let revision = self
            .apply_from(&canvas_id, doc.revision, ops, visibility::ambient_actor())
            .await?;
        Ok(json!({
            "action": "insert_image",
            "canvas_id": canvas_id,
            "shape_id": shape_id,
            "asset_id": asset_id,
            "replaced_frame": replaced_frame,
            "revision": revision,
        }))
    }

    async fn insert_html(&self, args: &CanvasToolArgs) -> Result<Value> {
        let canvas_id = Self::addressed(CanvasToolAction::InsertHtml, args)?;
        let doc = self.gate(CanvasToolAction::InsertHtml, &canvas_id).await?;
        let html = args.html.as_deref().filter(|h| !h.trim().is_empty());
        let Some(html) = html else {
            return Err(AlephError::tool(
                "insert_html: `html` is required (the single-file HTML body, inline)".to_string(),
            ));
        };
        let asset_id = self
            .store
            .put_asset(&canvas_id, "text/html", html.as_bytes())
            .await
            .map_err(|e| AlephError::tool(format!("insert_html: {e}")))?;

        let place = Self::placement_of(args, &doc, DEFAULT_HTML_FRAME)?;
        let mut ops = Vec::new();
        let (frame_id, replaced_child) = if let Some(frame) = &place.frame {
            // Replace mode: the target must actually be a frame — an html
            // child hung under a note would render nowhere the model expects.
            if !matches!(frame, Shape::Frame { .. }) {
                return Err(AlephError::tool(format!(
                    "insert_html: shape {} is not a frame — pass a Frame id, or omit `frame_id` \
                     to mint a new 16:9 frame",
                    frame.id()
                )));
            }
            let old_child = doc
                .shapes
                .iter()
                .find(|s| {
                    matches!(s, Shape::Html { .. })
                        && s.common().parent_id.as_deref() == Some(frame.id())
                })
                .map(|s| s.id().to_string());
            if let Some(old) = &old_child {
                ops.push(CanvasOp::DeleteShape { id: old.clone() });
            }
            (frame.id().to_string(), old_child)
        } else {
            let frame_id = mint_shape_id();
            ops.push(CanvasOp::UpsertShape {
                shape: Shape::Frame {
                    common: ShapeCommon {
                        id: frame_id.clone(),
                        x: place.x,
                        y: place.y,
                        w: place.w,
                        h: place.h,
                        z: place.z.clone(),
                        parent_id: None,
                    },
                    title: args.title.clone().unwrap_or_default(),
                    aspect_locked: true,
                },
            });
            (frame_id, None)
        };
        let child_id = mint_shape_id();
        ops.push(CanvasOp::UpsertShape {
            shape: Shape::Html {
                common: ShapeCommon {
                    id: child_id.clone(),
                    x: place.x,
                    y: place.y,
                    w: place.w,
                    h: place.h,
                    z: FracIndex::between(Some(&place.z), None),
                    parent_id: Some(frame_id.clone()),
                },
                asset_id: asset_id.clone(),
            },
        });
        let revision = self
            .apply_from(&canvas_id, doc.revision, ops, visibility::ambient_actor())
            .await?;
        Ok(json!({
            "action": "insert_html",
            "canvas_id": canvas_id,
            "frame_id": frame_id,
            "shape_id": child_id,
            "asset_id": asset_id,
            "replaced_child": replaced_child,
            "revision": revision,
        }))
    }

    async fn read_asset(&self, args: &CanvasToolArgs) -> Result<Value> {
        let canvas_id = Self::addressed(CanvasToolAction::ReadAsset, args)?;
        self.gate(CanvasToolAction::ReadAsset, &canvas_id).await?;
        let asset_id = args
            .asset_id
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty());
        let Some(asset_id) = asset_id else {
            return Err(AlephError::tool(
                "read_asset: `asset_id` is required (`<sha256>.<ext>`, as referenced by shapes)"
                    .to_string(),
            ));
        };
        let (mime, bytes) = self
            .store
            .read_asset(&canvas_id, asset_id)
            .await
            .map_err(|e| AlephError::tool(format!("read_asset: {e}")))?;
        if mime == "text/html" {
            return Ok(json!({
                "action": "read_asset",
                "canvas_id": canvas_id,
                "asset_id": asset_id,
                "mime_type": mime,
                "text": String::from_utf8_lossy(&bytes),
            }));
        }
        // An image goes out on the `_media` channel (artifact pane + channel
        // attachment), not inlined into the text result: the data URL is
        // resolved locally by the harvest, and the model gets the fact it
        // can act on — where the asset went — instead of a base64 wall.
        use base64::Engine as _;
        let data_url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );
        let item = crate::gateway::media::MediaItem {
            url: data_url,
            media_type: "image".to_string(),
            mime_type: Some(mime.clone()),
            filename: Some(asset_id.to_string()),
        };
        Ok(json!({
            "action": "read_asset",
            "canvas_id": canvas_id,
            "asset_id": asset_id,
            "mime_type": mime,
            "bytes": bytes.len(),
            "delivered": "attached as media for the user; the canvas already renders it",
            "_media": [serde_json::to_value(&item)?],
        }))
    }
}

/// Resolved placement for `insert_*`: a box, a stacking index, and (in
/// replace mode) the frame being targeted.
struct Placement {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    z: FracIndex,
    parent_id: Option<String>,
    frame: Option<Shape>,
}

/// The store's own not-found display, so "invisible" and "missing" are
/// byte-identical on this face.
fn not_found(canvas_id: &str) -> AlephError {
    AlephError::tool(CanvasError::NotFound(format!("canvas {canvas_id}")).to_string())
}

/// Mint a shape id in the store's id charset (`[A-Za-z0-9_-]`, hex here).
fn mint_shape_id() -> String {
    format!("s-{}", uuid::Uuid::new_v4().simple())
}

/// The serde `type` tag of a shape, for the summary listing.
///
/// A hand-written twin of the wire tag — pinned against the real serde
/// output by `summary_type_tags_match_the_wire_tags`, so it cannot drift
/// into a second vocabulary (§0: two spellings of one fact).
fn type_tag(shape: &Shape) -> &'static str {
    match shape {
        Shape::Geo { .. } => "geo",
        Shape::Ink { .. } => "ink",
        Shape::Text { .. } => "text",
        Shape::Note { .. } => "note",
        Shape::Image { .. } => "image",
        Shape::Frame { .. } => "frame",
        Shape::Html { .. } => "html",
        Shape::Arrow { .. } => "arrow",
        Shape::AiImageFrame { .. } => "ai_image_frame",
    }
}

/// The human text a shape carries, for the summary excerpt.
fn shape_text(shape: &Shape) -> &str {
    match shape {
        Shape::Geo { text, .. } | Shape::Text { text, .. } | Shape::Note { text, .. } => text,
        Shape::Arrow { label, .. } => label,
        Shape::Frame { title, .. } => title,
        Shape::AiImageFrame { prompt, .. } => prompt,
        Shape::Ink { .. } | Shape::Image { .. } | Shape::Html { .. } => "",
    }
}

fn excerpt(text: &str) -> String {
    text.chars().take(EXCERPT_CHARS).collect()
}

/// Token-lean projection for `get(detail="summary")`: ids, boxes, excerpts —
/// no ink points, no styles.
fn summary_of(doc: &CanvasDoc, selection: Vec<String>) -> Value {
    let shapes: Vec<Value> = doc
        .shapes
        .iter()
        .map(|s| {
            let c = s.common();
            let mut row = json!({
                "id": c.id,
                "type": type_tag(s),
                "x": c.x, "y": c.y, "w": c.w, "h": c.h,
            });
            let obj = row.as_object_mut().expect("literal object");
            if let Some(parent) = &c.parent_id {
                obj.insert("parent_id".into(), json!(parent));
            }
            let text = excerpt(shape_text(s));
            if !text.is_empty() {
                obj.insert("text_excerpt".into(), json!(text));
            }
            if let Shape::AiImageFrame { status, .. } = s {
                obj.insert("status".into(), json!(ai_status_tag(*status)));
            }
            row
        })
        .collect();
    json!({
        "action": "get",
        "id": doc.id,
        "title": doc.title,
        "revision": doc.revision,
        "selection": selection,
        "shapes": shapes,
        "decks": doc.decks,
    })
}

fn ai_status_tag(status: AiFrameStatus) -> &'static str {
    match status {
        AiFrameStatus::Draft => "draft",
        AiFrameStatus::Pending => "pending",
        AiFrameStatus::Done => "done",
        AiFrameStatus::Failed => "failed",
    }
}

/// Resolve `insert_image`'s `location` into `(mime, bytes)`.
///
/// Three forms, closed set:
/// - `data:` URL — decoded locally;
/// - local file path — ONLY under the canonicalized Aleph data dir or the OS
///   temp dir (both sides canonicalized before `starts_with`, §5.22 —
///   half-converted comparison flips verdicts on macOS's `/var` symlink);
/// - `https://` URL — 10 s timeout, streamed under `MAX_ASSET_BYTES`, and the
///   response must declare an `image/*` content type.
async fn resolve_image_location(location: &str) -> Result<(String, Vec<u8>)> {
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("data:") {
        return decode_data_url(location);
    }
    if lower.starts_with("https://") {
        return fetch_https_image(location).await;
    }
    if lower.starts_with("http://") {
        return Err(AlephError::tool(
            "insert_image: plain http:// is refused — use https://, a data: URL, or a local path"
                .to_string(),
        ));
    }
    read_local_image(location).await
}

fn decode_data_url(url: &str) -> Result<(String, Vec<u8>)> {
    use base64::Engine as _;
    let body = &url["data:".len()..];
    let Some((header, payload)) = body.split_once(',') else {
        return Err(AlephError::tool(
            "insert_image: malformed data: URL (no comma)".to_string(),
        ));
    };
    let Some(mime) = header.strip_suffix(";base64") else {
        return Err(AlephError::tool(
            "insert_image: only base64 data: URLs are supported (`data:<mime>;base64,…`)"
                .to_string(),
        ));
    };
    let mime = mime.trim().to_ascii_lowercase();
    if !mime.starts_with("image/") {
        return Err(AlephError::tool(format!(
            "insert_image: data: URL declares {mime:?}, expected an image/* type"
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| {
            AlephError::tool(format!("insert_image: data: URL is not valid base64: {e}"))
        })?;
    Ok((mime, bytes))
}

/// The image mime for a local file's extension — the same vocabulary as the
/// store's asset whitelist (`put_asset` re-verifies; this only picks the
/// spelling to hand it).
fn mime_for_local_ext(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

async fn read_local_image(location: &str) -> Result<(String, Vec<u8>)> {
    // Canonicalize BOTH sides before comparing — get_data_dir() may sit
    // behind a symlink (macOS /var → /private/var), and a half-converted
    // starts_with flips from admit to refuse or back (§5.22).
    let canonical = tokio::fs::canonicalize(location)
        .await
        .map_err(|e| AlephError::tool(format!("insert_image: cannot read {location:?}: {e}")))?;
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(data_dir) = crate::utils::paths::get_data_dir() {
        if let Ok(root) = std::fs::canonicalize(&data_dir) {
            roots.push(root);
        }
    }
    if let Ok(root) = std::fs::canonicalize(std::env::temp_dir()) {
        roots.push(root);
    }
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        return Err(AlephError::tool(
            "insert_image: local paths are only read from the Aleph data dir or the OS temp dir \
             (where generation tools write); pass a data: URL for anything else"
                .to_string(),
        ));
    }
    let Some(mime) = mime_for_local_ext(&canonical) else {
        return Err(AlephError::tool(format!(
            "insert_image: {location:?} has no recognized image extension \
             (png/jpg/jpeg/webp/gif/svg)"
        )));
    };
    let meta = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| AlephError::tool(format!("insert_image: cannot stat {location:?}: {e}")))?;
    if meta.len() > MAX_ASSET_BYTES as u64 {
        return Err(AlephError::tool(format!(
            "insert_image: {location:?} is {} bytes, over the {MAX_ASSET_BYTES}-byte asset cap",
            meta.len()
        )));
    }
    let bytes = tokio::fs::read(&canonical)
        .await
        .map_err(|e| AlephError::tool(format!("insert_image: cannot read {location:?}: {e}")))?;
    Ok((mime.to_string(), bytes))
}

async fn fetch_https_image(url: &str) -> Result<(String, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_FETCH_TIMEOUT)
        .build()
        .map_err(|e| AlephError::tool(format!("insert_image: http client: {e}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| AlephError::tool(format!("insert_image: fetch failed: {e}")))?;
    if !response.status().is_success() {
        return Err(AlephError::tool(format!(
            "insert_image: {url} answered HTTP {}",
            response.status()
        )));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if !mime.starts_with("image/") {
        return Err(AlephError::tool(format!(
            "insert_image: {url} served content type {mime:?}, expected image/*"
        )));
    }
    if let Some(len) = response.content_length() {
        if len > MAX_ASSET_BYTES as u64 {
            return Err(AlephError::tool(format!(
                "insert_image: {url} declares {len} bytes, over the {MAX_ASSET_BYTES}-byte cap"
            )));
        }
    }
    // Streamed cap: the bound is enforced WHILE reading, not measured after
    // the body already sat in memory (discovery.rs::read_bounded precedent).
    let mut response = response;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if buf.len() + chunk.len() > MAX_ASSET_BYTES {
                    return Err(AlephError::tool(format!(
                        "insert_image: {url} exceeded the {MAX_ASSET_BYTES}-byte cap mid-stream"
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(AlephError::tool(format!(
                    "insert_image: transport error reading {url}: {e}"
                )))
            }
        }
    }
    Ok((mime, buf))
}

#[async_trait]
impl AlephTool for CanvasTool {
    const NAME: &'static str = "canvas";
    /// Runtime facts only — the schema already carries the argument shapes.
    /// What is here is what no field doc can hold: the live Panel link, the
    /// frame-replacement semantics, the summary/full trade, the location
    /// roots, and that revisions are handled internally.
    const DESCRIPTION: &'static str = r#"Shared whiteboard canvases. The user sees every change live in the Panel's Canvas view, and their edits arrive the same way — treat a canvas as a shared surface, not a private buffer.

get returns a summary (boxes + 80-char text excerpts, no ink points or styles); ask detail="full" only when you need exact geometry. apply takes the same ops as the canvas.apply RPC (upsert_shape / delete_shape / set_doc_meta / upsert_deck / delete_deck); revisions are read and conflict-retried internally — never guess one.

insert_image accepts location as a data: URL, a local file path (only under the Aleph data dir or the OS temp dir — where generation tools write), or an https URL (10s, 10MB, must serve image/*). insert_html wraps the body in a 16:9 frame. For both, frame_id targets an existing frame: the image replaces the frame in place, the html replaces the frame's html child — that is how an AiImageFrame becomes its finished image. read_asset returns html as text; an image is attached as media for the user, not inlined.

Whole-canvas delete is Panel-only, deliberately."#;

    type Args = CanvasToolArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            CanvasToolAction::List => {
                let canvases: Vec<_> = self
                    .store
                    .list_entries()
                    .await
                    .into_iter()
                    .filter(|entry| {
                        visibility::ambient_canvas_visible(
                            entry.owner_user_id.as_deref(),
                            entry.row.project_id.as_deref(),
                        )
                    })
                    .map(|entry| entry.row)
                    .collect();
                Ok(json!({
                    "action": "list",
                    "count": canvases.len(),
                    "canvases": canvases,
                }))
            }
            CanvasToolAction::Create => {
                if let Some(project_id) = args.project_id.as_deref() {
                    // A project link WIDENS the audience to that roster, so
                    // the writer passes the same membership test the readers
                    // will — a room the caller cannot see refuses exactly
                    // like a room that does not exist.
                    if !visibility::project_visible_to(
                        project_id,
                        visibility::ambient_actor().as_deref(),
                    ) {
                        return Err(AlephError::tool(format!("not found: project {project_id}")));
                    }
                }
                let owner = crate::scope::ambient_owner();
                let doc = self
                    .store
                    .create(args.title.clone(), args.project_id.clone(), owner)
                    .await
                    .map_err(|e| AlephError::tool(format!("create: {e}")))?;
                Ok(json!({
                    "action": "create",
                    "canvas_id": doc.id,
                    "title": doc.title,
                    "revision": doc.revision,
                }))
            }
            CanvasToolAction::Get => {
                let canvas_id = Self::addressed(CanvasToolAction::Get, &args)?;
                let doc = self.gate(CanvasToolAction::Get, &canvas_id).await?;
                let selection = selection::get(&doc.id);
                match args.detail.as_deref().map(str::trim).unwrap_or("summary") {
                    "full" => Ok(json!({
                        "action": "get",
                        "canvas": doc,
                        "selection": selection,
                    })),
                    "summary" | "" => Ok(summary_of(&doc, selection)),
                    other => Err(AlephError::tool(format!(
                        "get: `detail` must be \"summary\" or \"full\" — got {other:?}"
                    ))),
                }
            }
            CanvasToolAction::Apply => {
                let canvas_id = Self::addressed(CanvasToolAction::Apply, &args)?;
                let doc = self.gate(CanvasToolAction::Apply, &canvas_id).await?;
                let Some(ops) = args.ops.clone().filter(|ops| !ops.is_empty()) else {
                    return Err(AlephError::tool(
                        "apply: `ops` is required (a non-empty op batch)".to_string(),
                    ));
                };
                let revision = self
                    .apply_from(&canvas_id, doc.revision, ops, visibility::ambient_actor())
                    .await?;
                Ok(json!({
                    "action": "apply",
                    "canvas_id": canvas_id,
                    "revision": revision,
                }))
            }
            CanvasToolAction::InsertImage => self.insert_image(&args).await,
            CanvasToolAction::InsertHtml => self.insert_html(&args).await,
            CanvasToolAction::ReadAsset => self.read_asset(&args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{with_scope, ScopeAttribution};
    use aleph_protocol::canvas::ShapeStyle;

    fn tool() -> (CanvasTool, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()));
        (CanvasTool::new(store), dir)
    }

    async fn call(t: &CanvasTool, v: Value) -> Result<Value> {
        t.call(serde_json::from_value(v).expect("args")).await
    }

    async fn call_as(t: &CanvasTool, user: &str, v: Value) -> Result<Value> {
        with_scope(Some(ScopeAttribution::personal(user)), call(t, v)).await
    }

    async fn create_as(t: &CanvasTool, user: &str) -> String {
        let created = call_as(t, user, json!({"action":"create","title":"T"}))
            .await
            .expect("create");
        created["canvas_id"]
            .as_str()
            .expect("canvas_id")
            .to_string()
    }

    fn note(id: &str, text: &str) -> Shape {
        Shape::Note {
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
            text: text.to_string(),
        }
    }

    fn upsert(shape: Shape) -> Value {
        serde_json::to_value(CanvasOp::UpsertShape { shape }).expect("op")
    }

    /// A 1×1 transparent PNG — small, valid base64, and enough for a store
    /// that addresses by content rather than decoding pixels.
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[tokio::test]
    async fn create_list_get_compose_for_the_owner() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;

        let listed = call_as(&t, "u-alice", json!({"action":"list"}))
            .await
            .expect("list");
        assert_eq!(listed["count"], 1);
        assert_eq!(listed["canvases"][0]["id"], id);

        // Summary is the default detail, and it reports revision + shapes.
        let got = call_as(&t, "u-alice", json!({"action":"get","canvas_id":id}))
            .await
            .expect("get");
        assert_eq!(got["id"], id);
        assert_eq!(got["revision"], 1);
        assert_eq!(got["shapes"].as_array().expect("shapes").len(), 0);
    }

    /// The no-oracle rule on this face: a stranger's refusal is byte-identical
    /// to a genuinely missing canvas, and the stranger's list is empty.
    #[tokio::test]
    async fn stranger_gets_not_found_shape() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;

        let refused = call_as(&t, "u-bob", json!({"action":"get","canvas_id":id}))
            .await
            .expect_err("stranger must be refused")
            .to_string();
        let missing = call_as(
            &t,
            "u-bob",
            json!({"action":"get","canvas_id":"cv-00000000000000000000000000000000"}),
        )
        .await
        .expect_err("missing must refuse")
        .to_string();
        // Same shape modulo the id each names.
        assert!(refused.contains("not found: canvas"), "{refused}");
        assert!(missing.contains("not found: canvas"), "{missing}");
        assert_eq!(
            refused.replace(&id, "<id>"),
            missing.replace("cv-00000000000000000000000000000000", "<id>"),
            "an invisible canvas must answer exactly like a missing one"
        );

        let listed = call_as(&t, "u-bob", json!({"action":"list"}))
            .await
            .expect("list");
        assert_eq!(listed["count"], 0, "{listed}");

        // The write faces refuse with the same shape — never a conflict.
        let write = call_as(
            &t,
            "u-bob",
            json!({"action":"apply","canvas_id":id,"ops":[upsert(note("n1","hi"))]}),
        )
        .await
        .expect_err("stranger apply must refuse")
        .to_string();
        assert!(write.contains("not found: canvas"), "{write}");
        assert!(!write.contains("revision"), "{write}");
    }

    #[tokio::test]
    async fn apply_needs_no_base_revision_and_bumps_the_doc() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let applied = call_as(
            &t,
            "u-alice",
            json!({"action":"apply","canvas_id":id,"ops":[upsert(note("n1","hello"))]}),
        )
        .await
        .expect("apply");
        assert_eq!(applied["revision"], 2);

        let got = call_as(
            &t,
            "u-alice",
            json!({"action":"get","canvas_id":id,"detail":"full"}),
        )
        .await
        .expect("get full");
        assert_eq!(got["canvas"]["shapes"][0]["id"], "n1");
        assert_eq!(got["canvas"]["shapes"][0]["text"], "hello");
    }

    /// The retry unit: a stale base conflicts once, and the retry lands
    /// against the revision the conflict itself named — no re-read.
    #[tokio::test]
    async fn apply_retries_once_on_conflict() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        // Bump the doc twice so revision 1 is stale.
        for n in ["n1", "n2"] {
            call_as(
                &t,
                "u-alice",
                json!({"action":"apply","canvas_id":id,"ops":[upsert(note(n,"x"))]}),
            )
            .await
            .expect("seed apply");
        }
        // First attempt from the stale base conflicts; the single retry wins.
        let revision = t
            .apply_from(
                &id,
                1,
                vec![CanvasOp::UpsertShape {
                    shape: note("n3", "late"),
                }],
                None,
            )
            .await
            .expect("one conflict retry must recover");
        assert_eq!(revision, 4);
    }

    #[tokio::test]
    async fn get_summary_excerpts_and_tags_shapes() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let long = "字".repeat(200);
        call_as(
            &t,
            "u-alice",
            json!({"action":"apply","canvas_id":id,"ops":[upsert(note("n1",&long))]}),
        )
        .await
        .expect("apply");
        let got = call_as(&t, "u-alice", json!({"action":"get","canvas_id":id}))
            .await
            .expect("get");
        let shape = &got["shapes"][0];
        assert_eq!(shape["type"], "note");
        let text = shape["text_excerpt"].as_str().expect("excerpt");
        assert_eq!(
            text.chars().count(),
            EXCERPT_CHARS,
            "the excerpt cap counts chars, not bytes"
        );
        assert!(shape.get("points").is_none(), "summary carries no payloads");
    }

    /// The hand-written summary tag vocabulary must be the wire's own —
    /// pinned against real serde output for every variant.
    #[test]
    fn summary_type_tags_match_the_wire_tags() {
        let common = ShapeCommon {
            id: "s1".into(),
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            z: FracIndex::first(),
            parent_id: None,
        };
        let shapes = vec![
            Shape::Geo {
                common: common.clone(),
                form: aleph_protocol::canvas::GeoForm::Rect,
                style: ShapeStyle::default(),
                text: String::new(),
            },
            Shape::Ink {
                common: common.clone(),
                style: ShapeStyle::default(),
                points: vec![],
            },
            Shape::Text {
                common: common.clone(),
                style: ShapeStyle::default(),
                text: String::new(),
            },
            Shape::Note {
                common: common.clone(),
                style: ShapeStyle::default(),
                text: String::new(),
            },
            Shape::Image {
                common: common.clone(),
                asset_id: "a".repeat(64) + ".png",
                natural_w: 0.0,
                natural_h: 0.0,
            },
            Shape::Frame {
                common: common.clone(),
                title: String::new(),
                aspect_locked: false,
            },
            Shape::Html {
                common: common.clone(),
                asset_id: "a".repeat(64) + ".html",
            },
            Shape::Arrow {
                common: common.clone(),
                start: aleph_protocol::canvas::ArrowEnd {
                    x: 0.0,
                    y: 0.0,
                    bind: None,
                },
                end: aleph_protocol::canvas::ArrowEnd {
                    x: 1.0,
                    y: 1.0,
                    bind: None,
                },
                style: ShapeStyle::default(),
                label: String::new(),
            },
            Shape::AiImageFrame {
                common,
                prompt: String::new(),
                reference_asset_ids: vec![],
                status: AiFrameStatus::Draft,
            },
        ];
        for shape in &shapes {
            let wire = serde_json::to_value(shape).expect("serialize");
            assert_eq!(
                wire["type"].as_str().expect("type tag"),
                type_tag(shape),
                "summary tag must be the wire tag"
            );
        }
    }

    #[tokio::test]
    async fn create_with_an_invisible_project_link_is_refused() {
        let (t, _dir) = tool();
        let err = call_as(
            &t,
            "u-bob",
            json!({"action":"create","project_id":"p-ghost"}),
        )
        .await
        .expect_err("foreign room must refuse")
        .to_string();
        assert!(err.contains("not found: project p-ghost"), "{err}");
    }

    #[tokio::test]
    async fn insert_image_data_url_lands_an_image_shape_and_asset() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let inserted = call_as(
            &t,
            "u-alice",
            json!({
                "action":"insert_image","canvas_id":id,
                "location": format!("data:image/png;base64,{TINY_PNG_B64}"),
            }),
        )
        .await
        .expect("insert_image");
        let asset_id = inserted["asset_id"].as_str().expect("asset id").to_string();
        assert!(asset_id.ends_with(".png"), "{asset_id}");
        assert!(inserted["replaced_frame"].is_null());

        let got = call_as(&t, "u-alice", json!({"action":"get","canvas_id":id}))
            .await
            .expect("get");
        assert_eq!(got["shapes"][0]["type"], "image");
        assert_eq!(got["shapes"][0]["x"], 100.0);

        let read = call_as(
            &t,
            "u-alice",
            json!({"action":"read_asset","canvas_id":id,"asset_id":asset_id}),
        )
        .await
        .expect("read_asset");
        assert_eq!(read["mime_type"], "image/png");
        let media = read["_media"].as_array().expect("_media");
        assert_eq!(media.len(), 1);
        assert!(
            media[0]["url"]
                .as_str()
                .expect("url")
                .starts_with("data:image/png;base64,"),
            "{read}"
        );
    }

    #[tokio::test]
    async fn insert_image_rejects_paths_outside_data_and_temp_roots() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        // /etc/hosts exists on every unix box and sits under neither root.
        let err = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_image","canvas_id":id,"location":"/etc/hosts"}),
        )
        .await
        .expect_err("a path outside the two roots must refuse")
        .to_string();
        assert!(err.contains("data dir"), "{err}");
        assert!(err.contains("temp dir"), "{err}");

        // Plain http is refused without any network reachability question.
        let err = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_image","canvas_id":id,
                   "location":"http://127.0.0.1:1/x.png"}),
        )
        .await
        .expect_err("http:// must refuse")
        .to_string();
        assert!(err.contains("https"), "{err}");
    }

    #[tokio::test]
    async fn insert_image_accepts_a_temp_dir_path() {
        use base64::Engine as _;
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        // A scratch file under the OS temp root — the guard lives to the end
        // of the test (§0: never drop the scratch guard before returning).
        let scratch = tempfile::TempDir::new().expect("scratch");
        let path = scratch.path().join("gen.png");
        std::fs::write(
            &path,
            base64::engine::general_purpose::STANDARD
                .decode(TINY_PNG_B64)
                .expect("png bytes"),
        )
        .expect("write scratch png");

        let inserted = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_image","canvas_id":id,
                   "location": path.to_string_lossy(),
                   "x": 10.0, "y": 20.0, "w": 64.0, "h": 64.0}),
        )
        .await
        .expect("temp-dir path must be admitted");
        assert!(inserted["asset_id"]
            .as_str()
            .expect("asset")
            .ends_with(".png"));

        let got = call_as(&t, "u-alice", json!({"action":"get","canvas_id":id}))
            .await
            .expect("get");
        assert_eq!(got["shapes"][0]["x"], 10.0);
        assert_eq!(got["shapes"][0]["w"], 64.0);
        drop(scratch);
    }

    #[tokio::test]
    async fn insert_image_replaces_a_frame_in_place() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let frame = Shape::AiImageFrame {
            common: ShapeCommon {
                id: "f1".into(),
                x: 10.0,
                y: 20.0,
                w: 300.0,
                h: 200.0,
                z: FracIndex::first(),
                parent_id: None,
            },
            prompt: "a cat".into(),
            reference_asset_ids: vec![],
            status: AiFrameStatus::Pending,
        };
        call_as(
            &t,
            "u-alice",
            json!({"action":"apply","canvas_id":id,"ops":[upsert(frame)]}),
        )
        .await
        .expect("seed frame");

        let inserted = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_image","canvas_id":id,"frame_id":"f1",
                   "location": format!("data:image/png;base64,{TINY_PNG_B64}")}),
        )
        .await
        .expect("insert into frame");
        assert_eq!(inserted["replaced_frame"], "f1");

        let got = call_as(&t, "u-alice", json!({"action":"get","canvas_id":id}))
            .await
            .expect("get");
        let shapes = got["shapes"].as_array().expect("shapes");
        assert_eq!(shapes.len(), 1, "the frame is gone: {got}");
        assert_eq!(shapes[0]["type"], "image");
        assert_eq!(shapes[0]["x"], 10.0);
        assert_eq!(shapes[0]["w"], 300.0);

        // A frame id that names nothing refuses by name.
        let err = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_image","canvas_id":id,"frame_id":"ghost",
                   "location": format!("data:image/png;base64,{TINY_PNG_B64}")}),
        )
        .await
        .expect_err("missing frame must refuse")
        .to_string();
        assert!(err.contains("not found: shape ghost"), "{err}");
    }

    #[tokio::test]
    async fn insert_html_wraps_in_a_16x9_frame_with_a_child() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let inserted = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_html","canvas_id":id,
                   "html":"<h1>hi</h1>","title":"Slide 1"}),
        )
        .await
        .expect("insert_html");
        let frame_id = inserted["frame_id"].as_str().expect("frame id");
        let child_id = inserted["shape_id"].as_str().expect("child id");
        let asset_id = inserted["asset_id"].as_str().expect("asset id").to_string();
        assert!(asset_id.ends_with(".html"));

        let got = call_as(
            &t,
            "u-alice",
            json!({"action":"get","canvas_id":id,"detail":"full"}),
        )
        .await
        .expect("get full");
        let shapes = got["canvas"]["shapes"].as_array().expect("shapes");
        assert_eq!(shapes.len(), 2);
        let frame = shapes.iter().find(|s| s["id"] == frame_id).expect("frame");
        assert_eq!(frame["type"], "frame");
        assert_eq!(frame["aspect_locked"], true);
        assert_eq!(frame["w"], 960.0);
        assert_eq!(frame["h"], 540.0);
        assert_eq!(frame["title"], "Slide 1");
        let child = shapes.iter().find(|s| s["id"] == child_id).expect("child");
        assert_eq!(child["type"], "html");
        assert_eq!(child["parent_id"], frame_id);

        let read = call_as(
            &t,
            "u-alice",
            json!({"action":"read_asset","canvas_id":id,"asset_id":asset_id}),
        )
        .await
        .expect("read_asset html");
        assert_eq!(read["text"], "<h1>hi</h1>");
        assert!(read.get("_media").is_none(), "html is text, not media");
    }

    #[tokio::test]
    async fn insert_html_frame_id_replaces_the_existing_child() {
        let (t, _dir) = tool();
        let id = create_as(&t, "u-alice").await;
        let first = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_html","canvas_id":id,"html":"<p>v1</p>"}),
        )
        .await
        .expect("first insert");
        let frame_id = first["frame_id"].as_str().expect("frame id").to_string();
        let old_child = first["shape_id"].as_str().expect("child id").to_string();

        let second = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_html","canvas_id":id,
                   "frame_id":frame_id,"html":"<p>v2</p>"}),
        )
        .await
        .expect("replace insert");
        assert_eq!(second["frame_id"], frame_id);
        assert_eq!(second["replaced_child"], old_child);

        let got = call_as(
            &t,
            "u-alice",
            json!({"action":"get","canvas_id":id,"detail":"full"}),
        )
        .await
        .expect("get full");
        let shapes = got["canvas"]["shapes"].as_array().expect("shapes");
        assert_eq!(shapes.len(), 2, "one frame, one (new) child: {got}");
        assert!(shapes
            .iter()
            .all(|s| s["id"] != serde_json::json!(old_child)));

        // Replace mode refuses a non-frame target by naming the mistake.
        call_as(
            &t,
            "u-alice",
            json!({"action":"apply","canvas_id":id,"ops":[upsert(note("n1","x"))]}),
        )
        .await
        .expect("seed note");
        let err = call_as(
            &t,
            "u-alice",
            json!({"action":"insert_html","canvas_id":id,
                   "frame_id":"n1","html":"<p>v3</p>"}),
        )
        .await
        .expect_err("a note is not a frame")
        .to_string();
        assert!(err.contains("not a frame"), "{err}");
    }

    /// Every addressed action refuses a missing canvas_id by naming itself —
    /// the model sees one tool, not seven.
    #[tokio::test]
    async fn every_addressed_action_refuses_a_missing_canvas_id_by_name() {
        let (t, _dir) = tool();
        for action in ["get", "apply", "insert_image", "insert_html", "read_asset"] {
            let err = match call(&t, json!({ "action": action })).await {
                Ok(v) => panic!("{action} must require a canvas_id, returned {v}"),
                Err(e) => e.to_string(),
            };
            assert!(
                err.contains(&format!("{action}: `canvas_id` is required")),
                "the refusal must name the action that wanted the id: {err}"
            );
        }
    }

    /// `action` is a closed enum: a verb the tool does not have fails schema
    /// validation instead of falling through to a default arm.
    #[test]
    fn an_unknown_action_fails_to_parse() {
        assert!(serde_json::from_value::<CanvasToolArgs>(
            json!({"action":"delete","canvas_id":"cv-x"})
        )
        .is_err());
    }
}

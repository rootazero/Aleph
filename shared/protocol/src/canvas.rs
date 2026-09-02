//! Whiteboard canvas wire contract — the single shape shared by the gateway
//! handlers (which BUILD responses from these types), the Panel, and the
//! `canvas` builtin tool. NOT `json_canvas.rs` (Obsidian interchange).
//! Key-set-equality tests live server-side in src/gateway/handlers/canvas.rs.
//!
//! Serde discipline (same as `workspace.rs`): optional request fields carry
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`; response
//! collection fields carry NO default (a missing key is a protocol error, not
//! an empty list); `CanvasDoc.decks` / `owner_user_id` / `project_id` DO carry
//! defaults so pre-deck documents on disk keep parsing.

use serde::{Deserialize, Serialize};

/// Gateway event topic carrying [`CanvasUpdated`] payloads.
pub const TOPIC: &str = "canvas.updated";
/// Upper bound for one stored asset (bytes — the dimension is bytes, not
/// lines: CWE-400 lesson).
pub const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;
/// Tighter bound for `text/html` assets (sandboxed iframe payloads).
pub const MAX_HTML_ASSET_BYTES: usize = 2 * 1024 * 1024;
/// Maximum shapes in one document; `canvas.apply` rejects past this.
pub const MAX_SHAPES: usize = 5000;
/// Maximum ops accepted by a single `canvas.apply` call.
pub const MAX_OPS_PER_APPLY: usize = 500;
/// Upper bound for a canvas title, in bytes.
///
/// Bytes, not characters, and named for it: the bound exists because the
/// title is stored, listed and broadcast, and those costs are byte-shaped.
/// 200 bytes is ~66 CJK characters — a title, not a document.
pub const MAX_TITLE_BYTES: usize = 200;

/// Upper bound for the serialized JSON of an entire canvas document.
///
/// The per-shape / per-asset / per-op caps already stop any one field from
/// becoming unbounded, but the AGGREGATE document was not capped: a member
/// could submit 5000 shapes whose `text` / `prompt` / `label` fields each
/// held tens of KB, producing a multi-hundred-MB JSON blob that the server
/// must deserialize, persist and broadcast. This is the byte ceiling the
/// serializer checks after every apply, so a single apply can never push
/// the document past it.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// The single gate over `CanvasDoc.title`, shared by every writer.
///
/// `title` has exactly two writers — the optional title of `canvas.create`
/// and [`CanvasOp::SetDocMeta`] — and until this existed neither checked
/// anything: every other stored dimension (shapes, ops per batch, ink points,
/// asset bytes) carried a cap while the one string a human reads carried
/// none. It lives in the contract rather than server-side so the Panel
/// refuses the same strings without a round trip, and refuses them for the
/// same reasons.
///
/// The gate **rejects, it never rewrites**: `apply_ops` stores what it was
/// given, so a normalizing gate would mean the value on disk is not the value
/// the caller sent. Trimming belongs at the input edge (the Panel's rename
/// helper trims before calling this).
///
/// Returns [`TitleRejection`] on refusal — an enum rather than a sentence,
/// because both consumers need a different rendering of the same fact and one
/// of them is localized. See that type.
pub fn check_title(title: &str) -> Result<(), TitleRejection> {
    if title.trim().is_empty() {
        return Err(TitleRejection::Empty);
    }
    if title.len() > MAX_TITLE_BYTES {
        return Err(TitleRejection::TooLong);
    }
    if title.chars().any(char::is_control) {
        return Err(TitleRejection::ControlCharacter);
    }
    Ok(())
}

/// Why [`check_title`] refused.
///
/// A closed enum, not the sentence itself, because the refusal has two
/// audiences that cannot share one string: the server hands it to models and
/// logs in English (via [`Display`](std::fmt::Display)), and the Panel shows
/// it to a person in their own language. A `&'static str` would have forced
/// the Panel either to display English or to pattern-match on English prose —
/// and a fourth reason added here would then render as nothing at all, in
/// silence. As an enum, the Panel's mapping is an exhaustive `match`: the
/// next variant is a compile error on every surface that renders one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TitleRejection {
    /// Blank, or nothing but whitespace.
    Empty,
    /// Over [`MAX_TITLE_BYTES`].
    TooLong,
    /// Carries a control character — a newline would break the single-line
    /// row the title is now navigated by, and is the shape that forges
    /// structure in text a model reads back.
    ControlCharacter,
}

impl std::fmt::Display for TitleRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("canvas title must not be empty"),
            Self::TooLong => write!(f, "canvas title exceeds {MAX_TITLE_BYTES} bytes"),
            Self::ControlCharacter => {
                f.write_str("canvas title must not contain control characters")
            }
        }
    }
}

/// Lexicographic fractional index over `0-9A-Za-z` (ASCII order of the digits
/// matches their rank, so plain string comparison is the z-order). `first()`
/// seeds at `"U"`; `between(None, None)` lands on the alphabet midpoint.
/// Digits append when a gap closes — length grows O(inserts-at-same-gap) and
/// never rebalances (a rebalance would rewrite sibling rows, defeating the
/// point). `between` never mints an index ending in `0`, and callers must not
/// hand-craft one: an index ending in the least digit has no room below it at
/// any longer length.
/// Hard upper bound on a fractional index length, in bytes. An index longer
/// than this on the wire is rejected at deserialisation: the type only
/// guarantees `between()` for indices that fit in this length, and a
/// hand-crafted (or attacker-supplied) long index would silently bypass the
/// "no trailing zero" invariant the z-order relies on.
pub const MAX_FRAC_INDEX_LEN: usize = 64;

impl<'de> Deserialize<'de> for FracIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        validate_frac_index(&raw).map_err(serde::de::Error::custom)?;
        Ok(Self(raw))
    }
}

fn validate_frac_index(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err("fractional index must not be empty".into());
    }
    if raw.len() > MAX_FRAC_INDEX_LEN {
        return Err(format!(
            "fractional index exceeds {MAX_FRAC_INDEX_LEN} bytes"
        ));
    }
    if raw.as_bytes().iter().any(|b| !DIGITS.contains(b)) {
        return Err(
            "fractional index must use only 0-9, A-Z, a-z (no punctuation, no unicode)"
                .into(),
        );
    }
    if raw.ends_with('0') {
        // An index ending in `0` has no room below it at any longer length
        // (`between` can never widen the gap), so accepting it on the wire
        // would let a hand-crafted value strangle the z-order at the very
        // edge — and once persisted, there is no recovery.
        return Err("fractional index must not end with '0'".into());
    }
    Ok(())
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct FracIndex(String);

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

impl FracIndex {
    /// The conventional seed index for the first shape on a canvas.
    #[must_use]
    pub fn first() -> Self {
        Self("U".to_string())
    }

    /// The raw digit string (what goes on the wire).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Midpoint strictly between `lo` and `hi` (either side open). Both
    /// bounds present requires `lo < hi`.
    #[must_use]
    pub fn between(lo: Option<&Self>, hi: Option<&Self>) -> Self {
        if let (Some(lo), Some(hi)) = (lo, hi) {
            debug_assert!(lo < hi, "between() requires lo < hi: {lo:?} vs {hi:?}");
        }
        let a = lo.map_or("", |f| f.0.as_str()).as_bytes();
        let b = hi.map_or("", |f| f.0.as_str()).as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let da = a.get(i).map_or(0i32, |c| digit_of(*c)); // missing low digit reads 0
            let db = b.get(i).map_or(62i32, |c| digit_of(*c)); // missing high digit reads 62 (one past 'z')
            if db - da > 1 {
                out.push(DIGITS[((da + db) / 2) as usize]);
                return Self(String::from_utf8(out).expect("ascii"));
            }
            out.push(DIGITS[da.max(0) as usize]);
            i += 1;
        }
    }
}

fn digit_of(c: u8) -> i32 {
    DIGITS.iter().position(|d| *d == c).map_or(0, |p| p as i32)
}

/// Fields every shape variant shares; `#[serde(flatten)]` lifts them onto the
/// top level of the tagged JSON object (same layout as `json_canvas.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapeCommon {
    /// Shape id, unique within its canvas.
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Z-order fractional index; higher sorts on top.
    pub z: FracIndex,
    /// Containing [`Shape::Frame`] id, when the shape lives inside a frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeoForm {
    Rect,
    Ellipse,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SizeKind {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapeStyle {
    /// Named palette slot ("default","red","orange","yellow","green","blue","violet")
    /// — resolved to theme tokens Panel-side; never a hex literal on the wire.
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub fill: bool,
    #[serde(default)]
    pub size: SizeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArrowEnd {
    pub x: f64,
    pub y: f64,
    /// Bound shape id; when present x/y are the recomputed fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiFrameStatus {
    Draft,
    Pending,
    Done,
    Failed,
}

/// One shape on the canvas. Internally tagged (`"type"`); an unknown tag is a
/// parse error, never a silent drop — a client that cannot represent a shape
/// must not quietly delete it on the next round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Geo {
        #[serde(flatten)]
        common: ShapeCommon,
        form: GeoForm,
        #[serde(default)]
        style: ShapeStyle,
        #[serde(default)]
        text: String,
    },
    Ink {
        #[serde(flatten)]
        common: ShapeCommon,
        #[serde(default)]
        style: ShapeStyle,
        /// `[x, y, pressure]` triplets, relative to the shape origin.
        points: Vec<[f32; 3]>,
    },
    Text {
        #[serde(flatten)]
        common: ShapeCommon,
        #[serde(default)]
        style: ShapeStyle,
        text: String,
    },
    Note {
        #[serde(flatten)]
        common: ShapeCommon,
        #[serde(default)]
        style: ShapeStyle,
        #[serde(default)]
        text: String,
    },
    Image {
        #[serde(flatten)]
        common: ShapeCommon,
        asset_id: String,
        #[serde(default)]
        natural_w: f64,
        #[serde(default)]
        natural_h: f64,
    },
    Frame {
        #[serde(flatten)]
        common: ShapeCommon,
        #[serde(default)]
        title: String,
        #[serde(default)]
        aspect_locked: bool,
    },
    Html {
        #[serde(flatten)]
        common: ShapeCommon,
        asset_id: String,
    },
    Arrow {
        #[serde(flatten)]
        common: ShapeCommon,
        start: ArrowEnd,
        end: ArrowEnd,
        #[serde(default)]
        style: ShapeStyle,
        #[serde(default)]
        label: String,
    },
    AiImageFrame {
        #[serde(flatten)]
        common: ShapeCommon,
        prompt: String,
        #[serde(default)]
        reference_asset_ids: Vec<String>,
        status: AiFrameStatus,
    },
}

impl Shape {
    #[must_use]
    pub fn common(&self) -> &ShapeCommon {
        match self {
            Self::Geo { common, .. }
            | Self::Ink { common, .. }
            | Self::Text { common, .. }
            | Self::Note { common, .. }
            | Self::Image { common, .. }
            | Self::Frame { common, .. }
            | Self::Html { common, .. }
            | Self::Arrow { common, .. }
            | Self::AiImageFrame { common, .. } => common,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.common().id
    }

    /// Asset ids this shape references (orphan-GC walks this).
    #[must_use]
    pub fn asset_ids(&self) -> Vec<&str> {
        match self {
            Self::Image { asset_id, .. } | Self::Html { asset_id, .. } => vec![asset_id],
            Self::AiImageFrame {
                reference_asset_ids,
                ..
            } => reference_asset_ids.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }
}

/// A slide deck: an ordered list of [`Shape::Frame`] ids on the same canvas.
/// Playing a deck never copies content — the frames ARE the slides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Deck {
    pub id: String,
    pub title: String,
    /// Presentation order; `frame_ids` itself is the sequence (no FracIndex).
    pub frame_ids: Vec<String>,
}

/// One whiteboard document — the unit of persistence (`doc.json`) and of
/// optimistic concurrency (`revision`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanvasDoc {
    /// `cv-<uuid>` minted by the store on create.
    pub id: String,
    pub title: String,
    /// Creator; `None` on legacy single-user installs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    /// Optional project-room link; roster members of that project see the
    /// canvas (`canvas_visible_to`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Optimistic-lock counter: `canvas.apply` carries the base and the store
    /// rejects a stale one with the current value.
    pub revision: u64,
    pub shapes: Vec<Shape>,
    /// Defaulted for forward compatibility: documents written before decks
    /// existed have no key and must keep parsing.
    #[serde(default)]
    pub decks: Vec<Deck>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// One mutation inside `canvas.apply`. Internally tagged (`"op"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CanvasOp {
    /// Insert or replace-in-place by `shape.id()`.
    UpsertShape {
        shape: Shape,
    },
    DeleteShape {
        id: String,
    },
    SetDocMeta {
        title: String,
    },
    /// Insert or replace-in-place by `deck.id`.
    UpsertDeck {
        deck: Deck,
    },
    DeleteDeck {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// RPC DTOs — the gateway handlers BUILD responses from these types (oversend
// is a compile error, not a hoped-for assertion), and the Panel parses them.
// ---------------------------------------------------------------------------

/// Parameters for `canvas.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Parameters for `canvas.get` / `canvas.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasRef {
    pub canvas_id: String,
}

/// Parameters for `canvas.apply` — the only write entry point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasApplyParams {
    pub canvas_id: String,
    pub base_revision: u64,
    pub ops: Vec<CanvasOp>,
}

/// Response of `canvas.apply`: the revision the ops landed as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasApplyResult {
    pub revision: u64,
}

/// One row of the canvas library listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasRow {
    pub id: String,
    pub title: String,
    pub revision: u64,
    /// Fixed-width on the wire (wasm32 clients have a 32-bit usize).
    pub shape_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub updated_at_ms: i64,
}

/// Response of `canvas.list`. `canvases` has no serde default on purpose: a
/// response that omits the key is a protocol error, and reading it as "no
/// canvases" would dress a broken server as an empty library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasList {
    pub canvases: Vec<CanvasRow>,
}

/// Response of `canvas.get`: the document plus the live selection and, once
/// the capability asset route exists, the base URL assets resolve against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasEnvelope {
    pub canvas: CanvasDoc,
    /// Most recently pushed selection for this canvas (shape ids); answers
    /// the model's "what did the user select".
    pub selection: Vec<String>,
    /// Capability-scoped base URL for `<image href>`; `None` until the asset
    /// route is wired (and on tool-face reads, which use base64 instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_base: Option<String>,
}

/// Parameters for `canvas.asset.put` (`data` is base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssetPutParams {
    pub canvas_id: String,
    pub mime_type: String,
    pub data: String,
}

/// Response of `canvas.asset.put`: content-addressed `<sha256>.<ext>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssetPutResult {
    pub asset_id: String,
}

/// Parameters for `canvas.asset.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssetGetParams {
    pub canvas_id: String,
    pub asset_id: String,
}

/// Response of `canvas.asset.get` (`data` is base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssetGetResult {
    pub mime_type: String,
    pub data: String,
}

/// Parameters for `canvas.selection.set` — the Panel pushes its selection so
/// the model can read it back through `canvas.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SelectionSetParams {
    pub canvas_id: String,
    pub shape_ids: Vec<String>,
}

/// Payload of the `canvas.updated` event as the Panel parses it. The
/// server-side frame sends owner/project fields alongside for visibility
/// classification; serde tolerates the extras here (no `deny_unknown_fields`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasUpdated {
    pub canvas_id: String,
    pub revision: u64,
    pub ops: Vec<CanvasOp>,
    /// Who applied the batch (user id or agent label); `None` for anonymous
    /// local sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_index_between_is_strictly_ordered() {
        let a = FracIndex::first();
        let b = FracIndex::between(Some(&a), None);
        let c = FracIndex::between(Some(&a), Some(&b));
        assert!(
            a < c && c < b,
            "between() must land strictly inside the gap"
        );
    }

    #[test]
    fn frac_index_repeated_inserts_stay_bounded_and_ordered() {
        // 1000 head inserts must not go unordered, and length must not
        // explode. Head-inserting IS "inserting at the same gap", the
        // documented worst case: each level yields four halvings
        // (V→F→7→3→1) and then one extension, so the measured maximum is
        // exactly 1 + 1000/5 = 201 chars. The bound pins that constant
        // factor — a regression to one-char-per-insert growth (1000)
        // still fails loudly.
        let mut hi = FracIndex::first();
        let mut prev_len = 0usize;
        for _ in 0..1000 {
            let lo = FracIndex::between(None, Some(&hi));
            assert!(lo < hi);
            prev_len = prev_len.max(lo.as_str().len());
            hi = lo;
        }
        assert!(prev_len <= 201, "index length must not explode: {prev_len}");
    }

    #[test]
    fn a_shape_round_trips_with_type_tag_and_flattened_common() {
        let s = Shape::Note {
            common: ShapeCommon {
                id: "n1".into(),
                x: 1.0,
                y: 2.0,
                w: 200.0,
                h: 200.0,
                z: FracIndex::first(),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: "hi".into(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["type"], "note");
        assert_eq!(v["id"], "n1"); // flatten lifts common onto the top level
        let back: Shape = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn an_unknown_shape_type_fails_to_parse_rather_than_silently_dropping() {
        let v = serde_json::json!({"type":"hologram","id":"x","x":0,"y":0,"w":1,"h":1,"z":"a1"});
        assert!(serde_json::from_value::<Shape>(v).is_err());
    }

    #[test]
    fn absent_optionals_are_omitted_rather_than_sent_as_null() {
        let p = CanvasCreateParams {
            title: None,
            project_id: None,
        };
        assert_eq!(serde_json::to_value(&p).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn a_doc_without_decks_key_parses_as_empty_decks() {
        // Forward compatibility: old doc.json has no decks key.
        let v = serde_json::json!({"id":"cv-1","title":"t","revision":1,
            "shapes":[],"created_at_ms":0,"updated_at_ms":0});
        let d: CanvasDoc = serde_json::from_value(v).unwrap();
        assert!(d.decks.is_empty() && d.owner_user_id.is_none());
    }

    // ---- manual sanity checks the plan requires, pinned as tests ----

    #[test]
    fn between_adjacent_digits_extends_with_a_longer_string() {
        // The plan's hand check: between("U", "V") has no one-digit answer, so
        // it must extend — and land strictly inside.
        let u = FracIndex::between(Some(&frac("U")), Some(&frac("V")));
        assert!(frac("U") < u && u < frac("V"), "got {:?}", u.as_str());
        assert!(u.as_str().len() > 1, "no single digit fits between U and V");
    }

    #[test]
    fn between_above_the_greatest_digit_still_finds_room() {
        // The upper open bound pads with 62 (one past 'z'), not 61 — otherwise
        // nothing could ever be minted after an index that is all-'z'.
        let z = frac("z");
        let after = FracIndex::between(Some(&z), None);
        assert!(z < after, "got {:?}", after.as_str());
    }

    #[test]
    fn an_op_round_trips_with_its_tag() {
        // Pins the wire tag key ("op") — the Panel's undo stack and the tool
        // face both re-serialize ops through this single source.
        let op = CanvasOp::DeleteShape { id: "n1".into() };
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!(v, serde_json::json!({"op":"delete_shape","id":"n1"}));
        let back: CanvasOp = serde_json::from_value(v).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn an_updated_event_tolerates_extra_frame_fields() {
        // The server-side frame sends owner/project alongside; the Panel
        // parses this payload and must tolerate the extras.
        let v = serde_json::json!({
            "canvas_id":"cv-1","revision":2,"ops":[],
            "owner_user_id":"u1","project_id":"p1"
        });
        let e: CanvasUpdated = serde_json::from_value(v).unwrap();
        assert_eq!(e.canvas_id, "cv-1");
        assert_eq!(e.revision, 2);
        assert!(e.actor.is_none());
    }

    fn frac(s: &str) -> FracIndex {
        serde_json::from_value(serde_json::json!(s)).expect("transparent string")
    }

    /// Wire-bound fractional index invariants: the type only guarantees
    /// `between()` for indices that fit in `MAX_FRAC_INDEX_LEN`, only use the
    /// 62 ASCII digits, and never end in `0` (the minimum digit, which has
    /// no room to widen). Accepting hand-crafted (or attacker-supplied) values
    /// that violate any of these on the wire would silently break z-order
    /// forever once persisted.
    #[test]
    fn frac_index_deserialization_rejects_bad_values() {
        for bad in ["", "path/to", "abc!", "with space", "0", "U0"] {
            let err = serde_json::from_value::<FracIndex>(serde_json::json!(bad))
                .err()
                .unwrap_or_else(|| panic!("expected {bad:?} to fail to deserialize"));
            assert!(
                err.to_string().to_lowercase().contains("fractional index"),
                "bad={bad:?} err={err}"
            );
        }
        // Past MAX_FRAC_INDEX_LEN.
        let too_long = "UUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUUU";
        assert!(
            serde_json::from_value::<FracIndex>(serde_json::json!(too_long)).is_err()
        );
        // Unicode / multi-byte punctuation must fail.
        let unicode = "Ué";
        assert!(
            serde_json::from_value::<FracIndex>(serde_json::json!(unicode)).is_err()
        );
    }

    #[test]
    fn frac_index_deserialization_accepts_valid_values() {
        for good in ["U", "Uu", "Ub", "z", "1", "V", "version1"] {
            serde_json::from_value::<FracIndex>(serde_json::json!(good))
                .unwrap_or_else(|e| panic!("good={good:?} should deserialize: {e}"));
        }
    }

    /// The cap counts bytes because it is named for bytes. A CJK title well
    /// under the character budget a byte-blind reader would assume must still
    /// pass, and a byte-over one must not — the failure mode this pins is the
    /// opposite of the `min_user_chars` lesson (a threshold named in one unit
    /// and measured in another), so it is asserted in both directions.
    #[test]
    fn the_title_cap_is_measured_in_the_unit_it_is_named_for() {
        let cjk = "\u{753b}".repeat(60); // 180 bytes, 60 chars
        assert_eq!(cjk.len(), 180);
        assert!(
            check_title(&cjk).is_ok(),
            "60 CJK chars is a title, not a document"
        );

        let too_long = "a".repeat(MAX_TITLE_BYTES + 1);
        assert!(check_title(&too_long).is_err());
    }

    /// Blank and control-bearing titles are refused. Whitespace-only matters
    /// because the Panel trims before sending: a title that trims to nothing
    /// would otherwise land as a row with no label at all.
    #[test]
    fn a_blank_or_control_bearing_title_is_refused_with_a_reason() {
        for bad in ["", "   ", "\u{9}\u{9}"] {
            assert!(check_title(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(
            check_title("one\ntwo"),
            Err(TitleRejection::ControlCharacter),
            "a newline is a control character"
        );
        assert!(
            TitleRejection::ControlCharacter
                .to_string()
                .contains("control"),
            "the English rendering names its cause so a model can self-heal"
        );
    }

    /// The gate refuses, it never rewrites — pinned because a normalizing
    /// gate would make the value on disk differ from the value the caller
    /// sent, silently, and `apply_ops` stores what it is handed.
    #[test]
    fn an_admissible_title_with_edge_whitespace_is_accepted_verbatim() {
        assert!(check_title(" spaced ").is_ok());
    }
}

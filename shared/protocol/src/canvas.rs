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
//!
//! The drawing-infrastructure fields (2026-09-12) follow the same two-tier
//! rule: `Option` fields (`reveal`, `timeline`) are skipped when absent, so an
//! old document re-serialises byte-identically; plain defaulted fields
//! (`stroke`, `bend`, `head_start`, `head_end`) are always written, so a shape
//! written by this build carries them explicitly. Both are pinned by
//! `an_old_document_parses_and_new_option_fields_stay_off_the_wire`.

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
            "fractional index must use only 0-9, A-Z, a-z (no punctuation, no unicode)".into(),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema)]
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
    /// When and how the shape appears during playback (`None` = always
    /// visible). Absent from the wire unless set, so documents without
    /// animation serialise exactly as they did before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<Reveal>,
}

/// Upper bound on any playback instant, in milliseconds (10 minutes): a
/// reveal must finish by it and a timeline must not run past it.
pub const MAX_REVEAL_MS: u32 = 600_000;

/// How a revealed shape comes in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RevealMode {
    /// The outline draws on along its path, then the fill/text fades in.
    #[default]
    Draw,
    Fade,
    Wipe,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    #[default]
    EaseOut,
    EaseInOut,
}

/// One shape's entrance during playback. Playback is the Panel's Play
/// button (and the animated export) replaying the document from t=0 — it is
/// draw-on for a recording, not a live animation running on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Reveal {
    /// Playback instant the entrance starts at.
    pub start_ms: u32,
    /// Length of the entrance; at least 1.
    pub duration_ms: u32,
    #[serde(default)]
    pub ease: Ease,
    #[serde(default)]
    pub mode: RevealMode,
}

impl Reveal {
    /// Bounds gate: a positive duration, and an end no later than
    /// [`MAX_REVEAL_MS`] (checked arithmetic — `u32::MAX + 1` must refuse,
    /// not wrap into an early instant).
    pub fn check(&self) -> Result<(), String> {
        if self.duration_ms == 0 {
            return Err("reveal duration_ms must be at least 1".to_string());
        }
        match self.start_ms.checked_add(self.duration_ms) {
            Some(end) if end <= MAX_REVEAL_MS => Ok(()),
            _ => Err(format!(
                "reveal must end by {MAX_REVEAL_MS} ms (start_ms + duration_ms)"
            )),
        }
    }
}

/// Document-level playback settings.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Timeline {
    /// Playback length; `None` = the latest reveal end.
    #[serde(default)]
    pub total_ms: Option<u32>,
    /// Pause on the finished frame before playback stops or loops.
    #[serde(default)]
    pub hold_ms: u32,
}

impl Timeline {
    /// Both instants within [`MAX_REVEAL_MS`].
    pub fn check(&self) -> Result<(), String> {
        if self.total_ms.is_some_and(|t| t > MAX_REVEAL_MS) {
            return Err(format!("timeline total_ms must not exceed {MAX_REVEAL_MS}"));
        }
        if self.hold_ms > MAX_REVEAL_MS {
            return Err(format!("timeline hold_ms must not exceed {MAX_REVEAL_MS}"));
        }
        Ok(())
    }

    /// The default timeline says nothing — a document never stores it
    /// (`SetDocMeta` with `Some(Timeline::default())` clears the field).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Upper bound on `Shape::Path.d`, in bytes.
pub const MAX_PATH_D_BYTES: usize = 64 * 1024;

/// One parsed path command, in absolute shape-local coordinates. Relative
/// commands and `H`/`V` are resolved during parsing, so no consumer ever
/// tracks a current point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo { x: f64, y: f64 },
    LineTo { x: f64, y: f64 },
    Quad { x1: f64, y1: f64, x: f64, y: f64 },
    Cubic { x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64 },
    Close,
}

impl PathCmd {
    /// Every coordinate the command carries (control points included) —
    /// what a bounds computation or a finiteness check walks. `Close`
    /// carries none.
    #[must_use]
    pub fn coords(&self) -> Vec<f64> {
        match *self {
            Self::MoveTo { x, y } | Self::LineTo { x, y } => vec![x, y],
            Self::Quad { x1, y1, x, y } => vec![x1, y1, x, y],
            Self::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => vec![x1, y1, x2, y2, x, y],
            Self::Close => Vec::new(),
        }
    }

    /// Where the pen is after this command; `None` for `Close` (the pen
    /// returns to the subpath start, which only the parser tracks).
    #[must_use]
    pub fn end_point(&self) -> Option<(f64, f64)> {
        match *self {
            Self::MoveTo { x, y }
            | Self::LineTo { x, y }
            | Self::Quad { x, y, .. }
            | Self::Cubic { x, y, .. } => Some((x, y)),
            Self::Close => None,
        }
    }
}

/// THE parser for `Shape::Path.d` — the server gate, the Panel renderer, the
/// export serialiser and the sketch synthesiser all read `d` through this
/// function, so there is exactly one answer to "what does this string mean".
///
/// Accepts the SVG path subset `M L H V Q C Z` (and their relative lower-case
/// forms) with SVG's implicit command repetition (`M 0 0 10 10` is a move
/// then a line; `L 1 2 3 4` is two lines). Rejects: empty input, a first
/// command other than `M`/`m`, any other command letter (`A` arcs, `S`/`T`
/// shorthands are deliberately outside the subset), a wrong argument count,
/// a non-finite number, and input over [`MAX_PATH_D_BYTES`].
pub fn parse_path_d(d: &str) -> Result<Vec<PathCmd>, String> {
    if d.len() > MAX_PATH_D_BYTES {
        return Err(format!(
            "path d is {} bytes, over the {MAX_PATH_D_BYTES}-byte cap",
            d.len()
        ));
    }
    let tokens = tokenize_path(d)?;
    if tokens.is_empty() {
        return Err("path d must not be empty".to_string());
    }
    let mut cmds = Vec::new();
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    let mut i = 0;
    let mut cmd: Option<char> = None;
    while i < tokens.len() {
        match tokens[i] {
            PathToken::Cmd(c) => {
                cmd = Some(c);
                i += 1;
                if c == 'Z' || c == 'z' {
                    cmds.push(PathCmd::Close);
                    (cx, cy) = (sx, sy);
                    continue;
                }
            }
            PathToken::Num(_) => {}
        }
        let Some(c) = cmd else {
            return Err("path d must start with a command letter (M or m)".to_string());
        };
        if c == 'Z' || c == 'z' {
            return Err("path d: numbers after Z (a command letter must follow)".to_string());
        }
        if cmds.is_empty() && c != 'M' && c != 'm' {
            return Err(format!("path d must start with M or m, not {c}"));
        }
        let arity = match c {
            'H' | 'h' | 'V' | 'v' => 1,
            'M' | 'm' | 'L' | 'l' => 2,
            'Q' | 'q' => 4,
            'C' | 'c' => 6,
            other => return Err(format!("path d: unsupported command {other:?}")),
        };
        let mut args = [0.0f64; 6];
        for (k, slot) in args.iter_mut().take(arity).enumerate() {
            match tokens.get(i + k) {
                Some(PathToken::Num(n)) => *slot = *n,
                _ => return Err(format!("path d: command {c} needs {arity} numbers")),
            }
        }
        i += arity;
        // A relative command is relative to the current point — except a
        // leading `m`, which SVG defines as absolute.
        let relative = c.is_ascii_lowercase() && !cmds.is_empty();
        let (ox, oy) = if relative { (cx, cy) } else { (0.0, 0.0) };
        let parsed = match c.to_ascii_uppercase() {
            'M' => PathCmd::MoveTo {
                x: ox + args[0],
                y: oy + args[1],
            },
            'L' => PathCmd::LineTo {
                x: ox + args[0],
                y: oy + args[1],
            },
            'H' => PathCmd::LineTo {
                x: ox + args[0],
                y: cy,
            },
            'V' => PathCmd::LineTo {
                x: cx,
                y: oy + args[0],
            },
            'Q' => PathCmd::Quad {
                x1: ox + args[0],
                y1: oy + args[1],
                x: ox + args[2],
                y: oy + args[3],
            },
            _ => PathCmd::Cubic {
                x1: ox + args[0],
                y1: oy + args[1],
                x2: ox + args[2],
                y2: oy + args[3],
                x: ox + args[4],
                y: oy + args[5],
            },
        };
        if !parsed.coords().iter().all(|v| v.is_finite()) {
            // Every token was finite, but a relative offset can still
            // overflow the current point.
            return Err("path d: coordinates must be finite".to_string());
        }
        if let Some((x, y)) = parsed.end_point() {
            if matches!(parsed, PathCmd::MoveTo { .. }) {
                (sx, sy) = (x, y);
            }
            (cx, cy) = (x, y);
        }
        cmds.push(parsed);
        // Implicit repetition: after a move, further pairs are lines.
        if c == 'M' {
            cmd = Some('L');
        } else if c == 'm' {
            cmd = Some('l');
        }
    }
    Ok(cmds)
}

/// The inverse of [`parse_path_d`]: absolute commands, space-separated, so a
/// parsed path re-emits as a string the parser reads back identically.
#[must_use]
pub fn cmds_to_d(cmds: &[PathCmd]) -> String {
    let mut out = String::new();
    for (i, cmd) in cmds.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        match cmd {
            PathCmd::MoveTo { x, y } => out.push_str(&format!("M{x} {y}")),
            PathCmd::LineTo { x, y } => out.push_str(&format!("L{x} {y}")),
            PathCmd::Quad { x1, y1, x, y } => out.push_str(&format!("Q{x1} {y1} {x} {y}")),
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => out.push_str(&format!("C{x1} {y1} {x2} {y2} {x} {y}")),
            PathCmd::Close => out.push('Z'),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PathToken {
    Cmd(char),
    Num(f64),
}

/// SVG path tokeniser: command letters and numbers, with commas and
/// whitespace as separators and the grammar's run-together forms
/// (`10-5`, `.5.5`, `1e-3`) split where SVG splits them. Every number must
/// parse finite — `1e999` is infinity and is refused here, before any
/// coordinate arithmetic.
fn tokenize_path(d: &str) -> Result<Vec<PathToken>, String> {
    let mut tokens = Vec::new();
    let mut num = String::new();
    let mut seen_dot = false;
    let mut seen_exp = false;
    let flush = |num: &mut String, tokens: &mut Vec<PathToken>| -> Result<(), String> {
        if num.is_empty() {
            return Ok(());
        }
        let value: f64 = num
            .parse()
            .map_err(|_| format!("path d: {num:?} is not a number"))?;
        if !value.is_finite() {
            return Err(format!("path d: {num:?} is not a finite number"));
        }
        tokens.push(PathToken::Num(value));
        num.clear();
        Ok(())
    };
    for ch in d.chars() {
        match ch {
            '0'..='9' => num.push(ch),
            '.' => {
                if seen_dot || seen_exp {
                    flush(&mut num, &mut tokens)?;
                    seen_exp = false;
                }
                seen_dot = true;
                num.push(ch);
            }
            '-' | '+' => {
                let after_exp = num.ends_with(['e', 'E']);
                if !after_exp {
                    flush(&mut num, &mut tokens)?;
                    seen_dot = false;
                    seen_exp = false;
                }
                num.push(ch);
            }
            'e' | 'E' if !num.is_empty() && !seen_exp => {
                seen_exp = true;
                num.push(ch);
            }
            ' ' | '\t' | '\n' | '\r' | ',' => {
                flush(&mut num, &mut tokens)?;
                seen_dot = false;
                seen_exp = false;
            }
            c if c.is_ascii_alphabetic() => {
                flush(&mut num, &mut tokens)?;
                seen_dot = false;
                seen_exp = false;
                tokens.push(PathToken::Cmd(c));
            }
            other => return Err(format!("path d: unexpected character {other:?}")),
        }
    }
    flush(&mut num, &mut tokens)?;
    Ok(tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeoForm {
    Rect,
    Ellipse,
    Diamond,
    Triangle,
    Hexagon,
    /// A stadium: a rect whose corner radius is half its height.
    Pill,
}

/// How a shape's outline is drawn. `Sketch` is a hand-drawn look the Panel
/// synthesises from the shape id (seeded jitter — the same document renders
/// the same everywhere); the dash patterns are plain SVG `stroke-dasharray`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StrokeKind {
    #[default]
    Solid,
    Sketch,
    Dashed,
    Dotted,
}

/// What an arrow end is capped with.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHead {
    #[default]
    None,
    Arrow,
    Triangle,
    Dot,
    Bar,
}

impl ArrowHead {
    /// The serde default of `Shape::Arrow.head_end` — an arrow with no heads
    /// asked for is still an arrow, so the end it points at gets one.
    #[must_use]
    pub const fn arrow() -> Self {
        Self::Arrow
    }
}

/// The seven named palette slots a `ShapeStyle.color` may name. The empty
/// string is also admitted (see [`check_color`]).
pub const PALETTE_SLOTS: [&str; 7] = [
    "default", "red", "orange", "yellow", "green", "blue", "violet",
];

/// The single gate over `ShapeStyle.color`, shared by every writer.
///
/// Admits the seven [`PALETTE_SLOTS`], a `#rrggbb` literal (exactly six hex
/// digits, either case), and the empty string. The empty string is not a
/// loophole: it is what `ShapeStyle::default()` mints and what every shape
/// written before this gate existed stores on disk — refusing it would make
/// those shapes immovable (a Panel drag re-upserts the shape verbatim). It
/// reads as the default slot on every renderer, exactly like `"default"`.
///
/// Rejects, never rewrites (the `check_title` rule): `"Red"` and `"red "` are
/// refused rather than normalised, so the value on disk is the value sent.
pub fn check_color(color: &str) -> Result<(), String> {
    if color.is_empty() || PALETTE_SLOTS.contains(&color) {
        return Ok(());
    }
    let hex = color.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(format!(
        "invalid color {color:?}: expected one of {} or #rrggbb",
        PALETTE_SLOTS.join("/")
    ))
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
    /// — resolved to theme tokens Panel-side — or a `#rrggbb` literal, used
    /// as-is. Gated by [`check_color`].
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub fill: bool,
    #[serde(default)]
    pub size: SizeKind,
    /// Outline treatment; serialised even at its default, so a shape written
    /// by this build always carries the key.
    #[serde(default)]
    pub stroke: StrokeKind,
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
        /// Curvature: signed perpendicular offset of the arc's midpoint from
        /// the straight chord, world units; `0` is a straight arrow.
        #[serde(default)]
        bend: f64,
        #[serde(default)]
        head_start: ArrowHead,
        #[serde(default = "ArrowHead::arrow")]
        head_end: ArrowHead,
    },
    /// Arbitrary vector outline. `d` is SVG path data restricted to
    /// `M L H V Q C Z` (absolute or relative), at most [`MAX_PATH_D_BYTES`],
    /// in coordinates relative to the shape's `x`/`y` (the `Ink` convention);
    /// [`parse_path_d`] is its one reader. `closed` fills the outline when
    /// the style asks for a fill.
    Path {
        #[serde(flatten)]
        common: ShapeCommon,
        #[serde(default)]
        style: ShapeStyle,
        d: String,
        #[serde(default)]
        closed: bool,
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
            | Self::Path { common, .. }
            | Self::AiImageFrame { common, .. } => common,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.common().id
    }

    /// The style of the variants that carry one (`Image`, `Frame`, `Html`
    /// and `AiImageFrame` have no user-set style).
    #[must_use]
    pub fn style(&self) -> Option<&ShapeStyle> {
        match self {
            Self::Geo { style, .. }
            | Self::Ink { style, .. }
            | Self::Text { style, .. }
            | Self::Note { style, .. }
            | Self::Arrow { style, .. }
            | Self::Path { style, .. } => Some(style),
            Self::Image { .. } | Self::Frame { .. } | Self::Html { .. } | Self::AiImageFrame { .. } => {
                None
            }
        }
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
    /// Playback settings; absent from the wire (and from disk) until set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Timeline>,
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
    /// `title` is always written. `timeline` is optional on the wire: `None`
    /// leaves the document's timeline untouched, `Some(t)` replaces it — and
    /// `Some(Timeline::default())` clears it, because the applier never
    /// stores the empty timeline (`Timeline::is_empty`).
    SetDocMeta {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline: Option<Timeline>,
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
                reveal: None,
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
        assert!(serde_json::from_value::<FracIndex>(serde_json::json!(too_long)).is_err());
        // Unicode / multi-byte punctuation must fail.
        let unicode = "Ué";
        assert!(serde_json::from_value::<FracIndex>(serde_json::json!(unicode)).is_err());
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

    // ---- drawing infrastructure (2026-09-12) --------------------------------

    /// Forward compatibility, both directions of the two-tier rule (module
    /// doc): a document written before any of the new fields existed parses,
    /// and re-serialises with the `Option` fields (`reveal`, `timeline`)
    /// still absent. The plain defaulted fields DO appear on re-serialise —
    /// `stroke`, `bend`, `head_start`, `head_end` — and this test pins that
    /// choice too, so nobody reads their presence as drift.
    #[test]
    fn an_old_document_parses_and_new_option_fields_stay_off_the_wire() {
        let old = serde_json::json!({
            "id":"cv-1","title":"t","revision":3,
            "shapes":[
                {"type":"geo","id":"g1","x":0,"y":0,"w":10,"h":10,"z":"U",
                 "form":"rect","style":{"color":"red","fill":true,"size":"medium"},"text":""},
                {"type":"arrow","id":"a1","x":0,"y":0,"w":10,"h":10,"z":"V",
                 "start":{"x":0,"y":0},"end":{"x":10,"y":10},"label":""}
            ],
            "decks":[],"created_at_ms":0,"updated_at_ms":0
        });
        let doc: CanvasDoc = serde_json::from_value(old).unwrap();
        assert!(doc.timeline.is_none());
        assert!(doc.shapes.iter().all(|s| s.common().reveal.is_none()));
        let Shape::Arrow {
            bend,
            head_start,
            head_end,
            ..
        } = &doc.shapes[1]
        else {
            panic!("second shape is the arrow");
        };
        assert_eq!((*bend, *head_start, *head_end), (0.0, ArrowHead::None, ArrowHead::Arrow));

        let back = serde_json::to_value(&doc).unwrap();
        assert!(back.get("timeline").is_none(), "{back}");
        for shape in back["shapes"].as_array().unwrap() {
            assert!(shape.get("reveal").is_none(), "{shape}");
        }
        assert_eq!(back["shapes"][0]["style"]["stroke"], "solid");
        assert_eq!(back["shapes"][1]["bend"], 0.0);
        assert_eq!(back["shapes"][1]["head_start"], "none");
        assert_eq!(back["shapes"][1]["head_end"], "arrow");
        // Everything the old document said is still said, verbatim.
        assert_eq!(back["id"], "cv-1");
        assert_eq!(back["revision"], 3);
        assert_eq!(back["shapes"][0]["style"]["color"], "red");
    }

    /// The wire spelling of every new enum variant, asserted explicitly —
    /// the snake_case convention is what the Panel and the model both write.
    #[test]
    fn new_enum_variants_spell_snake_case_on_the_wire() {
        fn wire<T: Serialize>(v: T) -> String {
            serde_json::to_value(v).unwrap().as_str().unwrap().to_string()
        }
        for (v, s) in [
            (GeoForm::Rect, "rect"),
            (GeoForm::Ellipse, "ellipse"),
            (GeoForm::Diamond, "diamond"),
            (GeoForm::Triangle, "triangle"),
            (GeoForm::Hexagon, "hexagon"),
            (GeoForm::Pill, "pill"),
        ] {
            assert_eq!(wire(v), s);
        }
        for (v, s) in [
            (StrokeKind::Solid, "solid"),
            (StrokeKind::Sketch, "sketch"),
            (StrokeKind::Dashed, "dashed"),
            (StrokeKind::Dotted, "dotted"),
        ] {
            assert_eq!(wire(v), s);
        }
        for (v, s) in [
            (ArrowHead::None, "none"),
            (ArrowHead::Arrow, "arrow"),
            (ArrowHead::Triangle, "triangle"),
            (ArrowHead::Dot, "dot"),
            (ArrowHead::Bar, "bar"),
        ] {
            assert_eq!(wire(v), s);
        }
        for (v, s) in [
            (RevealMode::Draw, "draw"),
            (RevealMode::Fade, "fade"),
            (RevealMode::Wipe, "wipe"),
        ] {
            assert_eq!(wire(v), s);
        }
        for (v, s) in [
            (Ease::Linear, "linear"),
            (Ease::EaseOut, "ease_out"),
            (Ease::EaseInOut, "ease_in_out"),
        ] {
            assert_eq!(wire(v), s);
        }
        // The Path variant's tag and its own fields.
        let v = serde_json::to_value(Shape::Path {
            common: ShapeCommon {
                id: "p1".into(),
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
                z: FracIndex::first(),
                parent_id: None,
                reveal: Some(Reveal {
                    start_ms: 10,
                    duration_ms: 20,
                    ease: Ease::default(),
                    mode: RevealMode::default(),
                }),
            },
            style: ShapeStyle::default(),
            d: "M0 0 L1 1".into(),
            closed: false,
        })
        .unwrap();
        assert_eq!(v["type"], "path");
        assert_eq!(v["d"], "M0 0 L1 1");
        assert_eq!(v["closed"], false);
        assert_eq!(v["reveal"]["ease"], "ease_out");
        assert_eq!(v["reveal"]["mode"], "draw");
    }

    #[test]
    fn parse_path_d_accepts_every_command_and_resolves_relative_forms() {
        let cmds = parse_path_d("M10 10 L20 10 H30 V20 Q35 25 40 20 C41 21 42 22 43 23 Z").unwrap();
        assert_eq!(
            cmds,
            vec![
                PathCmd::MoveTo { x: 10.0, y: 10.0 },
                PathCmd::LineTo { x: 20.0, y: 10.0 },
                PathCmd::LineTo { x: 30.0, y: 10.0 },
                PathCmd::LineTo { x: 30.0, y: 20.0 },
                PathCmd::Quad {
                    x1: 35.0,
                    y1: 25.0,
                    x: 40.0,
                    y: 20.0
                },
                PathCmd::Cubic {
                    x1: 41.0,
                    y1: 21.0,
                    x2: 42.0,
                    y2: 22.0,
                    x: 43.0,
                    y: 23.0
                },
                PathCmd::Close,
            ]
        );
        // Relative forms resolve against the current point; a leading `m`
        // is absolute (SVG), and `z` returns the pen to the subpath start.
        let rel = parse_path_d("m10 10 l5 0 h5 v5 q1 1 2 2 c1 1 2 2 3 3 z l1 1").unwrap();
        assert_eq!(
            rel,
            vec![
                PathCmd::MoveTo { x: 10.0, y: 10.0 },
                PathCmd::LineTo { x: 15.0, y: 10.0 },
                PathCmd::LineTo { x: 20.0, y: 10.0 },
                PathCmd::LineTo { x: 20.0, y: 15.0 },
                PathCmd::Quad {
                    x1: 21.0,
                    y1: 16.0,
                    x: 22.0,
                    y: 17.0
                },
                PathCmd::Cubic {
                    x1: 23.0,
                    y1: 18.0,
                    x2: 24.0,
                    y2: 19.0,
                    x: 25.0,
                    y: 20.0
                },
                PathCmd::Close,
                PathCmd::LineTo { x: 11.0, y: 11.0 },
            ]
        );
        // Implicit repetition and the grammar's run-together number forms.
        assert_eq!(
            parse_path_d("M0,0 10,10-5-5.5.5.25").unwrap(),
            vec![
                PathCmd::MoveTo { x: 0.0, y: 0.0 },
                PathCmd::LineTo { x: 10.0, y: 10.0 },
                PathCmd::LineTo { x: -5.0, y: -5.5 },
                PathCmd::LineTo { x: 0.5, y: 0.25 },
            ]
        );
        assert_eq!(
            parse_path_d("M0 0 L1e1 2E-1").unwrap()[1],
            PathCmd::LineTo { x: 10.0, y: 0.2 },
            "exponent forms are numbers, not commands"
        );
    }

    #[test]
    fn parse_path_d_refuses_what_is_outside_the_subset() {
        for (bad, why) in [
            ("", "empty"),
            ("   ", "blank"),
            ("L0 0", "must start with M"),
            ("M0 0 A5 5 0 0 1 10 10", "arcs are outside the subset"),
            ("M0 0 S1 1 2 2", "smooth curves are outside the subset"),
            ("M0 0 L1", "wrong argument count"),
            ("M0,0 10,10-5-5.5.5", "a lone trailing number"),
            ("M0 0 L1e999 0", "infinity"),
            ("M0 0 LNaN 0", "NaN"),
            ("M0 0 Z 1 1", "numbers after Z"),
            ("M0 0 L1 1; L2 2", "a stray character"),
        ] {
            assert!(parse_path_d(bad).is_err(), "{why}: {bad:?}");
        }
    }

    #[test]
    fn the_path_cap_is_flush_and_measured_in_bytes() {
        // A path exactly at the cap parses; one byte over does not — and the
        // over-cap refusal happens before the tokenizer reads anything.
        let unit = "L1 1 ";
        let body_len = MAX_PATH_D_BYTES - "M0 0 ".len();
        let mut d = String::from("M0 0 ");
        d.push_str(&unit.repeat(body_len / unit.len()));
        while d.len() < MAX_PATH_D_BYTES {
            d.push(' ');
        }
        assert_eq!(d.len(), MAX_PATH_D_BYTES);
        assert!(parse_path_d(&d).is_ok());
        d.push(' ');
        let err = parse_path_d(&d).unwrap_err();
        assert!(err.contains("byte"), "{err}");
    }

    #[test]
    fn cmds_to_d_round_trips_through_the_parser() {
        let src = "m1.5 2 l3 4 h-2 v0.25 q1 1 2 2 c0 0 1 1 2 2 z";
        let cmds = parse_path_d(src).unwrap();
        let d = cmds_to_d(&cmds);
        assert_eq!(parse_path_d(&d).unwrap(), cmds, "{d}");
        assert!(d.starts_with("M1.5 2 L4.5 6"), "absolute, space-separated: {d}");
        assert!(d.ends_with('Z'), "{d}");
    }

    #[test]
    fn check_color_admits_the_slots_and_six_digit_hex_only() {
        for slot in PALETTE_SLOTS {
            assert!(check_color(slot).is_ok(), "{slot}");
        }
        assert!(check_color("#a1B2c3").is_ok());
        assert!(check_color("#000000").is_ok());
        // The wire default — what `ShapeStyle::default()` mints and what every
        // pre-gate document stores. See the gate's doc for why refusing it
        // would break existing canvases.
        assert!(check_color("").is_ok());
        assert_eq!(ShapeStyle::default().color, "");
        for bad in ["#abc", "#gggggg", "red ", " red", "Red", "#a1b2c3d", "a1b2c3", "#"] {
            assert!(check_color(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn reveal_and_timeline_bounds_are_gated() {
        let ok = Reveal {
            start_ms: MAX_REVEAL_MS - 1,
            duration_ms: 1,
            ease: Ease::Linear,
            mode: RevealMode::Fade,
        };
        assert!(ok.check().is_ok());
        assert!(Reveal { duration_ms: 0, ..ok }.check().is_err(), "zero duration");
        assert!(
            Reveal {
                start_ms: MAX_REVEAL_MS,
                duration_ms: 1,
                ..ok
            }
            .check()
            .is_err(),
            "one past the ceiling"
        );
        assert!(
            Reveal {
                start_ms: u32::MAX,
                duration_ms: 2,
                ..ok
            }
            .check()
            .is_err(),
            "the sum must not wrap into an early instant"
        );

        assert!(Timeline::default().check().is_ok());
        assert!(Timeline::default().is_empty());
        assert!(Timeline {
            total_ms: Some(MAX_REVEAL_MS),
            hold_ms: MAX_REVEAL_MS
        }
        .check()
        .is_ok());
        assert!(Timeline {
            total_ms: Some(MAX_REVEAL_MS + 1),
            hold_ms: 0
        }
        .check()
        .is_err());
        assert!(Timeline {
            total_ms: None,
            hold_ms: MAX_REVEAL_MS + 1
        }
        .check()
        .is_err());
    }
}

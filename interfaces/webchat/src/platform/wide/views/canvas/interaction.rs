//! Pure pointer/keyboard interaction logic for the whiteboard's Select tool —
//! zero DOM, unit-tested on the native target. `editor.rs` wires DOM events
//! into [`InteractionState`] and executes the returned [`Effect`]s.
//!
//! # The machine's contract
//!
//! Every pointer entry point takes **world** coordinates (the editor converts
//! from screen through `viewport::screen_to_world`) and returns effects; the
//! machine never touches signals. Three effects exist:
//!
//! - [`Effect::SetSelection`] — replace the selection.
//! - [`Effect::Preview`] — tentative shape geometry during a drag. The editor
//!   applies these to the doc signal *locally only*: no undo entry, no send.
//! - [`Effect::Commit`] — a finished edit, carrying both directions. The
//!   machine computes `undo` from the originals it snapshotted at drag start,
//!   because by commit time the doc signal already holds the previewed
//!   geometry — inverting against it would "restore" the moved position
//!   (the reason this is not `ops::invert`'s job on drag commits).
//!
//! # Creation tools (Geo / Note / Text / Frame)
//!
//! A creation drag is its own entry point ([`InteractionState::begin_create`],
//! called by the editor when the active tool maps through [`create_kind`] —
//! `pointer_down` itself stays Select-only): down mints the id (injected, like
//! `duplicate_ops`) and stacks a fresh z above the document top; move previews
//! the dragged box, spanned by the two corners in any order so a reverse drag
//! normalizes to positive extents; up commits the shape (undo = delete) and
//! selects it, after which the editor drops the tool back to Select. A click
//! without a drag lands a default-sized shape centered on the point. The Text
//! tool is the exception: it never commits here — it previews the empty shape
//! and hands off to the text-editing overlay ([`Effect::BeginTextEdit`] with
//! `fresh: true`), whose empty-text commit discards instead of creating
//! (`text_edit.rs`).
//!
//! # Draw / Arrow (their own gesture shapes, not box drags)
//!
//! **Draw** ([`InteractionState::begin_draw`] / [`InteractionState::draw_move`])
//! accumulates world points with pressure, decimated to a minimum spacing the
//! editor derives from 2 screen px ÷ zoom; every accepted point re-previews
//! the ink shape, whose bbox is the points' extent and whose stored points
//! are origin-relative ([`Shape::Ink`]'s wire contract). The press itself
//! previews a dot, so a tap leaves a mark. Release commits (undo = delete)
//! without selecting — the tool stays armed, strokes come in batches.
//!
//! **Arrow** ([`InteractionState::begin_arrow`]) draws a two-point arrow from
//! press to release. A press over a shape binds `start`, a release over one
//! binds `end` ([`aleph_protocol::canvas::ArrowEnd::bind`]); the draft arrow
//! itself is excluded from binding (its preview lives in the doc and its own
//! bbox would otherwise swallow every hit). During the drag the machine
//! tracks the would-bind target ([`InteractionState::arrow_hover`]) so the
//! editor can highlight it. A click without a drag creates nothing.
//!
//! # Hit testing
//!
//! Rust-side bbox tests ([`topmost_hit`], marquee intersection) — the shapes'
//! axis-aligned bounds, topmost = greatest `(z, id)`, the exact tie-break
//! `editor.rs::z_sorted_ids` paints with (later-painted wins the hit).

use aleph_protocol::canvas::{
    ArrowEnd, CanvasOp, FracIndex, GeoForm, Shape, ShapeCommon, ShapeStyle,
};

use crate::state::canvas::CanvasTool;

/// A resize can never collapse a bbox below this many world units.
pub(super) const MIN_RESIZE: f64 = 1.0;
/// How far a duplicate (⌘D) lands from its source, world units.
pub(super) const DUPLICATE_OFFSET: f64 = 16.0;
/// Click-create default extents (world units), centered on the click point.
const CREATE_BOX_SIZE: (f64, f64) = (200.0, 200.0);
const CREATE_TEXT_SIZE: (f64, f64) = (240.0, 40.0);
/// Frames default 16:9 — the slide aspect (spec §4).
const CREATE_FRAME_SIZE: (f64, f64) = (640.0, 360.0);

/// Axis-aligned bounding box, world units. `w`/`h` are always ≥ 0 — every
/// constructor normalizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Bbox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bbox {
    /// Rectangle spanned by two corners, in any order.
    #[must_use]
    pub(super) fn from_corners(a: (f64, f64), b: (f64, f64)) -> Self {
        let (x0, x1) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };
        let (y0, y1) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
        Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// A shape's bounds — `common` x/y/w/h, normalized in case a producer
    /// ever writes a negative extent.
    #[must_use]
    pub(super) fn of_shape(shape: &Shape) -> Self {
        let c = shape.common();
        Self::from_corners((c.x, c.y), (c.x + c.w, c.y + c.h))
    }

    #[must_use]
    pub(super) fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.0 <= self.x + self.w && p.1 >= self.y && p.1 <= self.y + self.h
    }

    /// Closed-interval overlap — a marquee touching an edge selects.
    #[must_use]
    pub(super) fn intersects(&self, o: &Self) -> bool {
        self.x <= o.x + o.w
            && o.x <= self.x + self.w
            && self.y <= o.y + o.h
            && o.y <= self.y + self.h
    }

    #[must_use]
    pub(super) fn union(self, o: Self) -> Self {
        Self::from_corners(
            (self.x.min(o.x), self.y.min(o.y)),
            (
                (self.x + self.w).max(o.x + o.w),
                (self.y + self.h).max(o.y + o.h),
            ),
        )
    }
}

/// Union bbox of the selected shapes; `None` when the selection is empty (or
/// names only vanished shapes — a broadcast may have deleted them).
#[must_use]
pub(super) fn selection_bbox(shapes: &[Shape], ids: &[String]) -> Option<Bbox> {
    shapes
        .iter()
        .filter(|s| ids.iter().any(|id| id == s.id()))
        .map(Bbox::of_shape)
        .reduce(Bbox::union)
}

/// The selected shapes, in document order.
#[must_use]
pub(super) fn shapes_for(shapes: &[Shape], ids: &[String]) -> Vec<Shape> {
    shapes
        .iter()
        .filter(|s| ids.iter().any(|id| id == s.id()))
        .cloned()
        .collect()
}

/// Topmost shape under a world point: greatest `(z, id)` — the same
/// tie-break the painter uses, so the hit is what the user sees on top.
#[must_use]
pub(super) fn topmost_hit(shapes: &[Shape], world: (f64, f64)) -> Option<&Shape> {
    shapes
        .iter()
        .filter(|s| Bbox::of_shape(s).contains(world))
        .max_by(|a, b| {
            a.common()
                .z
                .cmp(&b.common().z)
                .then_with(|| a.id().cmp(b.id()))
        })
}

/// One of the eight resize handles, named by compass point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

impl Handle {
    pub(super) const ALL: [Self; 8] = [
        Self::Nw,
        Self::N,
        Self::Ne,
        Self::E,
        Self::Se,
        Self::S,
        Self::Sw,
        Self::W,
    ];

    /// Where this handle sits on a bbox, world units.
    #[must_use]
    pub(super) fn center(self, b: Bbox) -> (f64, f64) {
        let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        match self {
            Self::Nw => (b.x, b.y),
            Self::N => (cx, b.y),
            Self::Ne => (b.x + b.w, b.y),
            Self::E => (b.x + b.w, cy),
            Self::Se => (b.x + b.w, b.y + b.h),
            Self::S => (cx, b.y + b.h),
            Self::Sw => (b.x, b.y + b.h),
            Self::W => (b.x, cy),
        }
    }

    /// CSS cursor for hovering this handle.
    #[must_use]
    pub(super) fn cursor_css(self) -> &'static str {
        match self {
            Self::Nw | Self::Se => "nwse-resize",
            Self::Ne | Self::Sw => "nesw-resize",
            Self::N | Self::S => "ns-resize",
            Self::E | Self::W => "ew-resize",
        }
    }
}

/// New bbox after dragging `handle` to `world`. Corner handles move two
/// edges, edge handles one; dragging past the opposite edge flips (the
/// result normalizes), and no axis collapses below [`MIN_RESIZE`].
#[must_use]
pub(super) fn resize_bbox(origin: Bbox, handle: Handle, world: (f64, f64)) -> Bbox {
    let (mut x0, mut y0) = (origin.x, origin.y);
    let (mut x1, mut y1) = (origin.x + origin.w, origin.y + origin.h);
    match handle {
        Handle::Nw | Handle::W | Handle::Sw => x0 = world.0,
        Handle::Ne | Handle::E | Handle::Se => x1 = world.0,
        Handle::N | Handle::S => {}
    }
    match handle {
        Handle::Nw | Handle::N | Handle::Ne => y0 = world.1,
        Handle::Sw | Handle::S | Handle::Se => y1 = world.1,
        Handle::E | Handle::W => {}
    }
    let mut b = Bbox::from_corners((x0, y0), (x1, y1));
    if b.w < MIN_RESIZE {
        b.w = MIN_RESIZE;
    }
    if b.h < MIN_RESIZE {
        b.h = MIN_RESIZE;
    }
    b
}

/// Which shape a creation tool drags out. `Copy` so the drag state can hold
/// one without borrow gymnastics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateKind {
    Geo(GeoForm),
    Note,
    Text,
    Frame,
}

/// The creation kind for a tool, `None` for the non-creating tools
/// (Select/Pan) and for Draw/Arrow, which have their own entry points
/// ([`InteractionState::begin_draw`] / [`InteractionState::begin_arrow`] —
/// point accumulation and endpoint binding, not a box drag).
#[must_use]
pub(super) fn create_kind(tool: CanvasTool) -> Option<CreateKind> {
    match tool {
        CanvasTool::Geo(form) => Some(CreateKind::Geo(form)),
        CanvasTool::Note => Some(CreateKind::Note),
        CanvasTool::Text => Some(CreateKind::Text),
        CanvasTool::Frame => Some(CreateKind::Frame),
        CanvasTool::Select | CanvasTool::Pan | CanvasTool::Draw | CanvasTool::Arrow => None,
    }
}

/// Default extents when a creation tool is clicked rather than dragged.
#[must_use]
fn default_create_size(kind: CreateKind) -> (f64, f64) {
    match kind {
        CreateKind::Geo(_) | CreateKind::Note => CREATE_BOX_SIZE,
        CreateKind::Text => CREATE_TEXT_SIZE,
        CreateKind::Frame => CREATE_FRAME_SIZE,
    }
}

/// Neither creation axis may collapse below [`MIN_RESIZE`] — a perfectly
/// horizontal drag must still land a visible shape.
#[must_use]
fn min_extent(mut b: Bbox) -> Bbox {
    if b.w < MIN_RESIZE {
        b.w = MIN_RESIZE;
    }
    if b.h < MIN_RESIZE {
        b.h = MIN_RESIZE;
    }
    b
}

/// The freshly created shape for a kind: default style, empty text, no
/// parent. `z` is minted once per gesture (at `begin_create`) so the preview
/// upserts keep replacing the same shape instead of stacking copies.
#[must_use]
fn shape_for_create(kind: CreateKind, id: &str, z: &FracIndex, b: Bbox) -> Shape {
    let common = ShapeCommon {
        id: id.to_string(),
        x: b.x,
        y: b.y,
        w: b.w,
        h: b.h,
        z: z.clone(),
        parent_id: None,
    };
    match kind {
        CreateKind::Geo(form) => Shape::Geo {
            common,
            form,
            style: ShapeStyle::default(),
            text: String::new(),
        },
        CreateKind::Note => Shape::Note {
            common,
            style: ShapeStyle::default(),
            text: String::new(),
        },
        CreateKind::Text => Shape::Text {
            common,
            style: ShapeStyle::default(),
            text: String::new(),
        },
        CreateKind::Frame => Shape::Frame {
            common,
            title: String::new(),
            aspect_locked: false,
        },
    }
}

/// The ink shape for a set of accumulated **world** points: the bbox is the
/// points' extent (clamped to visible), and the stored points are relative
/// to that origin — the wire contract of [`Shape::Ink`]. Total on empty
/// input for defensiveness, though every producer seeds the press point.
#[must_use]
fn ink_shape(id: &str, z: &FracIndex, world_points: &[[f32; 3]]) -> Shape {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in world_points {
        min_x = min_x.min(f64::from(p[0]));
        min_y = min_y.min(f64::from(p[1]));
        max_x = max_x.max(f64::from(p[0]));
        max_y = max_y.max(f64::from(p[1]));
    }
    if world_points.is_empty() {
        (min_x, min_y, max_x, max_y) = (0.0, 0.0, 0.0, 0.0);
    }
    let b = min_extent(Bbox {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    });
    let points = world_points
        .iter()
        .map(|p| [p[0] - min_x as f32, p[1] - min_y as f32, p[2]])
        .collect();
    Shape::Ink {
        common: ShapeCommon {
            id: id.to_string(),
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            z: z.clone(),
            parent_id: None,
        },
        style: ShapeStyle::default(),
        points,
    }
}

/// A two-point arrow spanning `start`→`end`, endpoints absolute (the wire
/// contract of [`Shape::Arrow`]), bbox normalized over the two points.
#[must_use]
fn arrow_shape(
    id: &str,
    z: &FracIndex,
    start: (f64, f64),
    start_bind: Option<String>,
    end: (f64, f64),
    end_bind: Option<String>,
) -> Shape {
    let b = Bbox::from_corners(start, end);
    Shape::Arrow {
        common: ShapeCommon {
            id: id.to_string(),
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            z: z.clone(),
            parent_id: None,
        },
        start: ArrowEnd {
            x: start.0,
            y: start.1,
            bind: start_bind,
        },
        end: ArrowEnd {
            x: end.0,
            y: end.1,
            bind: end_bind,
        },
        style: ShapeStyle::default(),
        label: String::new(),
    }
}

/// The topmost shape under a world point that an arrow endpoint may bind to
/// — [`topmost_hit`]'s ordering, minus the draft arrow itself (its preview
/// lives in the doc, and its own bbox contains both endpoints).
#[must_use]
fn bind_target<'a>(shapes: &'a [Shape], world: (f64, f64), draft_id: &str) -> Option<&'a Shape> {
    shapes
        .iter()
        .filter(|s| s.id() != draft_id && Bbox::of_shape(s).contains(world))
        .max_by(|a, b| {
            a.common()
                .z
                .cmp(&b.common().z)
                .then_with(|| a.id().cmp(b.id()))
        })
}

/// Pointer pressure with the hardware-less fallback: mice (and pens that do
/// not report pressure) may deliver 0 even mid-press; the spec's "no
/// pressure support" value is 0.5, so a zero reading normalizes to it —
/// otherwise a mouse stroke would draw at the minimum radius everywhere.
#[must_use]
pub(super) fn effective_pressure(raw: f32) -> f32 {
    if raw <= 0.0 {
        0.5
    } else {
        raw
    }
}

fn common_mut(shape: &mut Shape) -> &mut ShapeCommon {
    match shape {
        Shape::Geo { common, .. }
        | Shape::Ink { common, .. }
        | Shape::Text { common, .. }
        | Shape::Note { common, .. }
        | Shape::Image { common, .. }
        | Shape::Frame { common, .. }
        | Shape::Html { common, .. }
        | Shape::Arrow { common, .. }
        | Shape::AiImageFrame { common, .. } => common,
    }
}

/// A shape translated by a world delta. Arrows carry absolute endpoint
/// coordinates outside `common`, so they move too; ink points are
/// origin-relative and ride along for free.
#[must_use]
pub(super) fn translate_shape(shape: &Shape, dx: f64, dy: f64) -> Shape {
    let mut s = shape.clone();
    {
        let c = common_mut(&mut s);
        c.x += dx;
        c.y += dy;
    }
    if let Shape::Arrow { start, end, .. } = &mut s {
        start.x += dx;
        start.y += dy;
        end.x += dx;
        end.y += dy;
    }
    s
}

#[must_use]
pub(super) fn translate_shapes(shapes: &[Shape], dx: f64, dy: f64) -> Vec<Shape> {
    shapes.iter().map(|s| translate_shape(s, dx, dy)).collect()
}

/// Map the originals through the affine that carries `from` onto `to`.
/// Positions map through the box transform; extents scale; ink points
/// (origin-relative) and arrow endpoints (absolute) scale on both axes.
#[must_use]
pub(super) fn scale_shapes(originals: &[Shape], from: Bbox, to: Bbox) -> Vec<Shape> {
    // A zero-extent axis cannot define a scale; treat it as 1:1 so the
    // orthogonal axis still resizes (a single hairline shape is the case).
    let sx = if from.w > 0.0 { to.w / from.w } else { 1.0 };
    let sy = if from.h > 0.0 { to.h / from.h } else { 1.0 };
    let fx = |x: f64| to.x + (x - from.x) * sx;
    let fy = |y: f64| to.y + (y - from.y) * sy;
    originals
        .iter()
        .map(|shape| {
            let mut s = shape.clone();
            {
                let c = common_mut(&mut s);
                c.x = fx(c.x);
                c.y = fy(c.y);
                c.w *= sx;
                c.h *= sy;
            }
            match &mut s {
                Shape::Ink { points, .. } => {
                    for p in points {
                        p[0] = (f64::from(p[0]) * sx) as f32;
                        p[1] = (f64::from(p[1]) * sy) as f32;
                    }
                }
                Shape::Arrow { start, end, .. } => {
                    start.x = fx(start.x);
                    start.y = fy(start.y);
                    end.x = fx(end.x);
                    end.y = fy(end.y);
                }
                _ => {}
            }
            s
        })
        .collect()
}

/// What the editor must do in response to a machine transition.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Effect {
    /// Replace the selection (the editor debounces the server push).
    SetSelection(Vec<String>),
    /// Tentative geometry during a drag — apply to the doc signal locally,
    /// no undo entry, no send.
    Preview(Vec<Shape>),
    /// A finished edit: optimistic-apply `redo`, record both on the undo
    /// stack, enqueue `redo` for the server.
    Commit {
        redo: Vec<CanvasOp>,
        undo: Vec<CanvasOp>,
    },
    /// Remove locally-previewed shapes that were never committed (an
    /// abandoned creation drag, a discarded fresh text shape) — the editor
    /// applies `DeleteShape` locally: no undo entry, no send. `Preview`
    /// cannot express this (it only carries upserts), and `Commit` must not
    /// (a rollback that lands on the undo stack would redo into existence).
    DiscardPreview(Vec<String>),
    /// Open the text-editing overlay over `shape`. `fresh` marks a shape
    /// born in this gesture and never committed: an empty-text commit (or
    /// Esc) discards it instead of writing an invisible empty shape —
    /// `text_edit::commit_outcome` owns that rule.
    BeginTextEdit { shape: Shape, fresh: bool },
}

/// The in-flight drag, if any.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) enum Drag {
    #[default]
    None,
    /// An ink stroke being drawn: world points accumulate (decimated to
    /// `min_dist`), each accepted point re-previews the shape.
    Drawing {
        id: String,
        /// Fresh z above the document top, minted once with the id.
        z: FracIndex,
        /// Accumulated **world** points (`[x, y, pressure]`); the shape they
        /// preview/commit normalizes them to its bbox ([`ink_shape`]).
        points: Vec<[f32; 3]>,
        /// Minimum world distance between successive points — the editor
        /// passes 2 screen px ÷ zoom.
        min_dist: f64,
    },
    /// A two-point arrow being dragged out.
    ArrowDraft {
        id: String,
        z: FracIndex,
        start: (f64, f64),
        /// Shape under the press, bound as the start endpoint.
        start_bind: Option<String>,
        /// Shape currently under the pointer — the endpoint's would-bind
        /// target, surfaced for hover highlighting.
        hover: Option<String>,
        /// Set once the pointer leaves the click slop; an arrow needs a
        /// drag — a click creates nothing.
        moved: bool,
    },
    Create {
        kind: CreateKind,
        /// Minted at `begin_create` — stable through the gesture, so every
        /// preview upsert replaces the same shape.
        id: String,
        /// Fresh z above the document top, minted once with the id.
        z: FracIndex,
        origin: (f64, f64),
        moved: bool,
    },
    Marquee {
        origin: (f64, f64),
        current: (f64, f64),
        /// Selection to add to (shift-marquee); empty for a plain marquee.
        base: Vec<String>,
    },
    Move {
        start: (f64, f64),
        /// Snapshot at drag start — both the rollback target (Esc) and the
        /// source of `undo` ops at commit.
        originals: Vec<Shape>,
        /// Set once the pointer leaves the click slop; a click that never
        /// moves commits nothing.
        moved: bool,
    },
    Resize {
        handle: Handle,
        origin_bbox: Bbox,
        originals: Vec<Shape>,
        moved: bool,
    },
}

/// The Select tool's pointer state machine.
#[derive(Debug, Default)]
pub(super) struct InteractionState {
    drag: Drag,
}

impl InteractionState {
    #[must_use]
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub(super) fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// The live marquee rectangle, for rendering.
    #[must_use]
    pub(super) fn marquee_rect(&self) -> Option<Bbox> {
        if let Drag::Marquee {
            origin, current, ..
        } = &self.drag
        {
            Some(Bbox::from_corners(*origin, *current))
        } else {
            None
        }
    }

    /// Pointer down on the canvas surface (not on a resize handle — that is
    /// [`Self::begin_resize`]).
    ///
    /// Select tool only; the creation tools enter through [`Self::begin_create`],
    /// Draw through [`Self::begin_draw`], Arrow through [`Self::begin_arrow`].
    /// Semantics: shift-click toggles membership without starting a drag;
    /// a plain click on a selected shape starts moving the whole selection;
    /// on an unselected shape it selects it and starts moving just it; on
    /// empty canvas it clears (or, with shift, keeps) the selection and
    /// starts a marquee.
    #[must_use]
    pub(super) fn pointer_down(
        &mut self,
        world: (f64, f64),
        hit: Option<&Shape>,
        selection: &[String],
        shapes: &[Shape],
        tool: CanvasTool,
        shift: bool,
    ) -> Vec<Effect> {
        if tool != CanvasTool::Select {
            return Vec::new();
        }
        match hit {
            Some(shape) => {
                if shift {
                    let mut sel = selection.to_vec();
                    if let Some(pos) = sel.iter().position(|id| id == shape.id()) {
                        sel.remove(pos);
                    } else {
                        sel.push(shape.id().to_string());
                    }
                    self.drag = Drag::None;
                    vec![Effect::SetSelection(sel)]
                } else if selection.iter().any(|id| id == shape.id()) {
                    self.drag = Drag::Move {
                        start: world,
                        originals: shapes_for(shapes, selection),
                        moved: false,
                    };
                    Vec::new()
                } else {
                    self.drag = Drag::Move {
                        start: world,
                        originals: vec![shape.clone()],
                        moved: false,
                    };
                    vec![Effect::SetSelection(vec![shape.id().to_string()])]
                }
            }
            None => {
                let base = if shift {
                    selection.to_vec()
                } else {
                    Vec::new()
                };
                let effects = if shift {
                    Vec::new()
                } else {
                    vec![Effect::SetSelection(Vec::new())]
                };
                self.drag = Drag::Marquee {
                    origin: world,
                    current: world,
                    base,
                };
                effects
            }
        }
    }

    /// Pointer down on a resize handle. `originals` is the current selection's
    /// shapes; an empty selection is a no-op (there is no handle to grab).
    #[must_use]
    pub(super) fn begin_resize(&mut self, handle: Handle, originals: Vec<Shape>) -> Vec<Effect> {
        let Some(origin_bbox) = originals.iter().map(Bbox::of_shape).reduce(Bbox::union) else {
            return Vec::new();
        };
        self.drag = Drag::Resize {
            handle,
            origin_bbox,
            originals,
            moved: false,
        };
        Vec::new()
    }

    /// Pointer down with a creation tool ([`create_kind`] `Some`). `id` is
    /// minted by the caller (`id_mint` is wasm-only; tests inject literals),
    /// `shapes` seeds the fresh z above the document top.
    ///
    /// Clears the selection — the gesture's focus is the shape being born,
    /// and a stale outline riding over the preview would fight it visually.
    #[must_use]
    pub(super) fn begin_create(
        &mut self,
        kind: CreateKind,
        world: (f64, f64),
        id: String,
        shapes: &[Shape],
    ) -> Vec<Effect> {
        let top = shapes.iter().map(|s| s.common().z.clone()).max();
        self.drag = Drag::Create {
            kind,
            id,
            z: FracIndex::between(top.as_ref(), None),
            origin: world,
            moved: false,
        };
        vec![Effect::SetSelection(Vec::new())]
    }

    /// Pointer down with the Draw tool. Seeds the stroke with the press
    /// point and previews it immediately — a tap must leave a dot, and the
    /// unconditional preview keeps cancel/commit total (something to
    /// discard, something to upsert). Clears the selection like every
    /// creation gesture.
    #[must_use]
    pub(super) fn begin_draw(
        &mut self,
        world: (f64, f64),
        pressure: f32,
        id: String,
        shapes: &[Shape],
        min_dist: f64,
    ) -> Vec<Effect> {
        let top = shapes.iter().map(|s| s.common().z.clone()).max();
        let z = FracIndex::between(top.as_ref(), None);
        let points = vec![[world.0 as f32, world.1 as f32, pressure]];
        let preview = ink_shape(&id, &z, &points);
        self.drag = Drag::Drawing {
            id,
            z,
            points,
            min_dist,
        };
        vec![
            Effect::SetSelection(Vec::new()),
            Effect::Preview(vec![preview]),
        ]
    }

    /// Pointer moved while drawing. `None` when no ink stroke is active (the
    /// caller falls through to [`Self::pointer_move`]); `Some(vec![])` when
    /// the point was decimated away.
    #[must_use]
    pub(super) fn draw_move(&mut self, world: (f64, f64), pressure: f32) -> Option<Vec<Effect>> {
        let Drag::Drawing {
            id,
            z,
            points,
            min_dist,
        } = &mut self.drag
        else {
            return None;
        };
        let last = points
            .last()
            .expect("a drawing always holds its press point");
        let (dx, dy) = (world.0 - f64::from(last[0]), world.1 - f64::from(last[1]));
        if (dx * dx + dy * dy).sqrt() < *min_dist {
            return Some(Vec::new());
        }
        points.push([world.0 as f32, world.1 as f32, pressure]);
        Some(vec![Effect::Preview(vec![ink_shape(id, z, points)])])
    }

    /// Pointer down with the Arrow tool. `hit` is the shape under the press
    /// (the editor's topmost hit) — it becomes the start endpoint's binding.
    #[must_use]
    pub(super) fn begin_arrow(
        &mut self,
        world: (f64, f64),
        hit: Option<&Shape>,
        id: String,
        shapes: &[Shape],
    ) -> Vec<Effect> {
        let top = shapes.iter().map(|s| s.common().z.clone()).max();
        self.drag = Drag::ArrowDraft {
            id,
            z: FracIndex::between(top.as_ref(), None),
            start: world,
            start_bind: hit.map(|s| s.id().to_string()),
            hover: None,
            moved: false,
        };
        vec![Effect::SetSelection(Vec::new())]
    }

    /// The shape an arrow endpoint would bind to right now — for the
    /// editor's hover highlight. `None` outside an arrow drag.
    #[must_use]
    pub(super) fn arrow_hover(&self) -> Option<String> {
        if let Drag::ArrowDraft { hover, .. } = &self.drag {
            hover.clone()
        } else {
            None
        }
    }

    /// Pointer moved. `slop` is the click tolerance in world units (the
    /// editor passes screen-pixels ÷ zoom): a Move drag stays a click until
    /// the pointer leaves it.
    #[must_use]
    pub(super) fn pointer_move(
        &mut self,
        world: (f64, f64),
        shapes: &[Shape],
        slop: f64,
    ) -> Vec<Effect> {
        match &mut self.drag {
            Drag::None => Vec::new(),
            // Drawing is pressure-driven and handled by `draw_move`; a stray
            // pressureless move (the editor never sends one) changes nothing.
            Drag::Drawing { .. } => Vec::new(),
            Drag::ArrowDraft {
                id,
                z,
                start,
                start_bind,
                hover,
                moved,
            } => {
                let (dx, dy) = (world.0 - start.0, world.1 - start.1);
                if !*moved && dx.abs() <= slop && dy.abs() <= slop {
                    return Vec::new();
                }
                *moved = true;
                *hover = bind_target(shapes, world, id).map(|s| s.id().to_string());
                vec![Effect::Preview(vec![arrow_shape(
                    id,
                    z,
                    *start,
                    start_bind.clone(),
                    world,
                    None,
                )])]
            }
            Drag::Create {
                kind,
                id,
                z,
                origin,
                moved,
            } => {
                let (dx, dy) = (world.0 - origin.0, world.1 - origin.1);
                if !*moved && dx.abs() <= slop && dy.abs() <= slop {
                    return Vec::new();
                }
                *moved = true;
                let b = min_extent(Bbox::from_corners(*origin, world));
                vec![Effect::Preview(vec![shape_for_create(*kind, id, z, b)])]
            }
            Drag::Move {
                start,
                originals,
                moved,
            } => {
                let (dx, dy) = (world.0 - start.0, world.1 - start.1);
                if !*moved && dx.abs() <= slop && dy.abs() <= slop {
                    return Vec::new();
                }
                *moved = true;
                vec![Effect::Preview(translate_shapes(originals, dx, dy))]
            }
            Drag::Marquee {
                origin,
                current,
                base,
            } => {
                *current = world;
                let rect = Bbox::from_corners(*origin, world);
                vec![Effect::SetSelection(marquee_selection(shapes, base, rect))]
            }
            Drag::Resize {
                handle,
                origin_bbox,
                originals,
                moved,
            } => {
                *moved = true;
                let to = resize_bbox(*origin_bbox, *handle, world);
                vec![Effect::Preview(scale_shapes(originals, *origin_bbox, to))]
            }
        }
    }

    /// Pointer released: commit the drag (or finalize the marquee).
    #[must_use]
    pub(super) fn pointer_up(&mut self, world: (f64, f64), shapes: &[Shape]) -> Vec<Effect> {
        match std::mem::take(&mut self.drag) {
            Drag::None => Vec::new(),
            Drag::Drawing { id, z, points, .. } => {
                // Commit whatever accumulated — a tap is a dot. No selection
                // change: the Draw tool stays armed, strokes come in batches.
                let shape = ink_shape(&id, &z, &points);
                vec![Effect::Commit {
                    redo: vec![CanvasOp::UpsertShape { shape }],
                    undo: vec![CanvasOp::DeleteShape { id }],
                }]
            }
            Drag::ArrowDraft {
                id,
                z,
                start,
                start_bind,
                moved,
                ..
            } => {
                if !moved {
                    // An arrow needs two distinct points; nothing previewed.
                    return Vec::new();
                }
                let end_bind = bind_target(shapes, world, &id).map(|s| s.id().to_string());
                let shape = arrow_shape(&id, &z, start, start_bind, world, end_bind);
                vec![
                    Effect::SetSelection(vec![id.clone()]),
                    Effect::Commit {
                        redo: vec![CanvasOp::UpsertShape { shape }],
                        undo: vec![CanvasOp::DeleteShape { id }],
                    },
                ]
            }
            Drag::Create {
                kind,
                id,
                z,
                origin,
                moved,
            } => {
                let b = if moved {
                    min_extent(Bbox::from_corners(origin, world))
                } else {
                    // A click: land the default size centered on the point
                    // that was pressed (not the release — the press is where
                    // the user aimed).
                    let (w, h) = default_create_size(kind);
                    Bbox {
                        x: origin.0 - w / 2.0,
                        y: origin.1 - h / 2.0,
                        w,
                        h,
                    }
                };
                let shape = shape_for_create(kind, &id, &z, b);
                let mut effects = vec![Effect::SetSelection(vec![id.clone()])];
                if kind == CreateKind::Text {
                    // Handed to the text editor uncommitted: an empty text
                    // shape is invisible, so nothing goes on the wire until
                    // the overlay commits non-empty text (or discards).
                    effects.push(Effect::Preview(vec![shape.clone()]));
                    effects.push(Effect::BeginTextEdit { shape, fresh: true });
                } else {
                    effects.push(Effect::Commit {
                        redo: vec![CanvasOp::UpsertShape { shape }],
                        undo: vec![CanvasOp::DeleteShape { id }],
                    });
                }
                effects
            }
            Drag::Move {
                start,
                originals,
                moved,
            } => {
                if !moved {
                    return Vec::new();
                }
                let (dx, dy) = (world.0 - start.0, world.1 - start.1);
                commit_replacing(translate_shapes(&originals, dx, dy), originals)
            }
            Drag::Marquee { origin, base, .. } => {
                let rect = Bbox::from_corners(origin, world);
                vec![Effect::SetSelection(marquee_selection(shapes, &base, rect))]
            }
            Drag::Resize {
                handle,
                origin_bbox,
                originals,
                moved,
            } => {
                if !moved {
                    return Vec::new();
                }
                let to = resize_bbox(origin_bbox, handle, world);
                commit_replacing(scale_shapes(&originals, origin_bbox, to), originals)
            }
        }
    }

    /// Esc (or pointercancel): abandon the drag and roll the preview back to
    /// the snapshot — the doc signal returns byte-identical to pre-drag.
    #[must_use]
    pub(super) fn cancel(&mut self) -> Vec<Effect> {
        match std::mem::take(&mut self.drag) {
            Drag::None => Vec::new(),
            // A drawing previews from the press itself — always discard.
            Drag::Drawing { id, .. } => vec![Effect::DiscardPreview(vec![id])],
            Drag::ArrowDraft { id, moved, .. } => {
                if moved {
                    vec![Effect::DiscardPreview(vec![id])]
                } else {
                    Vec::new()
                }
            }
            Drag::Create { id, moved, .. } => {
                if moved {
                    // The preview inserted a shape the doc never owned:
                    // rolling back means deleting it, not restoring it.
                    vec![Effect::DiscardPreview(vec![id])]
                } else {
                    Vec::new()
                }
            }
            Drag::Move {
                originals, moved, ..
            } => {
                if moved {
                    vec![Effect::Preview(originals)]
                } else {
                    Vec::new()
                }
            }
            Drag::Resize {
                originals, moved, ..
            } => {
                if moved {
                    vec![Effect::Preview(originals)]
                } else {
                    Vec::new()
                }
            }
            Drag::Marquee { base, .. } => vec![Effect::SetSelection(base)],
        }
    }
}

/// A drag commit: upsert the finals, undo by upserting the originals.
fn commit_replacing(finals: Vec<Shape>, originals: Vec<Shape>) -> Vec<Effect> {
    vec![Effect::Commit {
        redo: finals
            .into_iter()
            .map(|shape| CanvasOp::UpsertShape { shape })
            .collect(),
        undo: originals
            .into_iter()
            .map(|shape| CanvasOp::UpsertShape { shape })
            .collect(),
    }]
}

/// Marquee result: the shift base plus every shape whose bbox intersects the
/// rectangle, base order first, hits in document order, no duplicates.
#[must_use]
fn marquee_selection(shapes: &[Shape], base: &[String], rect: Bbox) -> Vec<String> {
    let mut sel = base.to_vec();
    for s in shapes {
        if Bbox::of_shape(s).intersects(&rect) && !sel.iter().any(|id| id == s.id()) {
            sel.push(s.id().to_string());
        }
    }
    sel
}

// ---------------------------------------------------------------------------
// Keyboard edit helpers — pure op producers the editor's key handler calls.
// Their `undo` side comes from `ops::invert` against the (clean, un-previewed)
// doc; the editor guards that no drag is active.
// ---------------------------------------------------------------------------

/// Delete the selection.
#[must_use]
pub(super) fn delete_ops(selection: &[String]) -> Vec<CanvasOp> {
    selection
        .iter()
        .map(|id| CanvasOp::DeleteShape { id: id.clone() })
        .collect()
}

/// Nudge the selection by a world delta (arrow keys).
#[must_use]
pub(super) fn nudge_ops(shapes: &[Shape], selection: &[String], dx: f64, dy: f64) -> Vec<CanvasOp> {
    shapes_for(shapes, selection)
        .iter()
        .map(|s| CanvasOp::UpsertShape {
            shape: translate_shape(s, dx, dy),
        })
        .collect()
}

/// Duplicate the selection (⌘D): clones offset by [`DUPLICATE_OFFSET`], with
/// minted ids and fresh z indexes stacked above the document's top so the
/// copies land in front. Returns the ops and the new selection.
///
/// `mint` is injected so tests stay deterministic and the wasm-only id mint
/// stays out of pure code.
#[must_use]
pub(super) fn duplicate_ops(
    shapes: &[Shape],
    selection: &[String],
    mint: &mut dyn FnMut() -> String,
) -> (Vec<CanvasOp>, Vec<String>) {
    let mut top_z = shapes.iter().map(|s| s.common().z.clone()).max();
    let mut ops = Vec::new();
    let mut new_ids = Vec::new();
    for original in shapes_for(shapes, selection) {
        let mut copy = translate_shape(&original, DUPLICATE_OFFSET, DUPLICATE_OFFSET);
        let z = FracIndex::between(top_z.as_ref(), None);
        {
            let c = common_mut(&mut copy);
            c.id = mint();
            c.z = z.clone();
            new_ids.push(c.id.clone());
        }
        top_z = Some(z);
        ops.push(CanvasOp::UpsertShape { shape: copy });
    }
    (ops, new_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{ArrowEnd, CanvasOp, ShapeStyle};

    fn frac(s: &str) -> FracIndex {
        serde_json::from_value(serde_json::json!(s)).expect("transparent string")
    }

    fn note_at(id: &str, x: f64, y: f64, w: f64, h: f64, z: &str) -> Shape {
        Shape::Note {
            common: ShapeCommon {
                id: id.to_string(),
                x,
                y,
                w,
                h,
                z: frac(z),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: String::new(),
        }
    }

    fn ids(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::SetSelection(sel) => Some(sel.clone()),
                _ => None,
            })
            .expect("expected a SetSelection effect")
    }

    fn sel(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    // ---- hit testing -------------------------------------------------------

    #[test]
    fn topmost_hit_prefers_the_greater_z_and_then_the_greater_id() {
        let shapes = vec![
            note_at("low", 0.0, 0.0, 100.0, 100.0, "K"),
            note_at("high", 50.0, 50.0, 100.0, 100.0, "V"),
            note_at("a-tied", 0.0, 0.0, 100.0, 100.0, "V"),
        ];
        // (75,75) is inside all three; "high" and "a-tied" tie on z, and the
        // greater id paints later, so it wins the hit.
        assert_eq!(topmost_hit(&shapes, (75.0, 75.0)).unwrap().id(), "high");
        // (10,10) is only inside the two full-origin boxes.
        assert_eq!(topmost_hit(&shapes, (10.0, 10.0)).unwrap().id(), "a-tied");
        assert!(topmost_hit(&shapes, (500.0, 500.0)).is_none());
    }

    // ---- click selection ---------------------------------------------------

    #[test]
    fn a_click_selects_the_hit_shape_and_arms_a_move() {
        let shapes = vec![note_at("a", 0.0, 0.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let effects = m.pointer_down(
            (10.0, 10.0),
            Some(&shapes[0]),
            &[],
            &shapes,
            CanvasTool::Select,
            false,
        );
        assert_eq!(ids(&effects), sel(&["a"]));
        assert!(m.is_dragging());
        // Released without moving: nothing commits.
        assert!(m.pointer_up((10.0, 10.0), &shapes).is_empty());
        assert!(!m.is_dragging());
    }

    #[test]
    fn shift_click_adds_to_and_removes_from_the_selection_without_dragging() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 100.0, 100.0, "U"),
            note_at("b", 200.0, 0.0, 100.0, 100.0, "V"),
        ];
        let mut m = InteractionState::new();
        let effects = m.pointer_down(
            (210.0, 10.0),
            Some(&shapes[1]),
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            true,
        );
        assert_eq!(ids(&effects), sel(&["a", "b"]), "shift-click adds");
        assert!(!m.is_dragging(), "shift-click adjusts, it does not drag");

        let effects = m.pointer_down(
            (10.0, 10.0),
            Some(&shapes[0]),
            &sel(&["a", "b"]),
            &shapes,
            CanvasTool::Select,
            true,
        );
        assert_eq!(ids(&effects), sel(&["b"]), "shift-click removes");
    }

    #[test]
    fn an_empty_click_clears_the_selection_and_a_non_select_tool_is_inert() {
        let shapes = vec![note_at("a", 0.0, 0.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let effects = m.pointer_down(
            (500.0, 500.0),
            None,
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            false,
        );
        assert_eq!(ids(&effects), Vec::<String>::new());
        assert!(m.marquee_rect().is_some(), "empty click arms a marquee");

        let mut inert = InteractionState::new();
        assert!(inert
            .pointer_down(
                (10.0, 10.0),
                Some(&shapes[0]),
                &[],
                &shapes,
                CanvasTool::Pan,
                false
            )
            .is_empty());
        assert!(!inert.is_dragging());
    }

    // ---- marquee -----------------------------------------------------------

    #[test]
    fn a_marquee_selects_by_bbox_intersection_live_and_on_release() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 100.0, 100.0, "U"),
            note_at("b", 300.0, 0.0, 100.0, 100.0, "V"),
            note_at("far", 1000.0, 1000.0, 10.0, 10.0, "W"),
        ];
        let mut m = InteractionState::new();
        let _ = m.pointer_down((90.0, -10.0), None, &[], &shapes, CanvasTool::Select, false);
        // A sliver over "a" only (intersection, not containment: overlapping
        // a's right edge is enough).
        let effects = m.pointer_move((95.0, 50.0), &shapes, 0.0);
        assert_eq!(ids(&effects), sel(&["a"]));
        // Extend right across "b" — both now intersect.
        let effects = m.pointer_move((350.0, 50.0), &shapes, 0.0);
        assert_eq!(ids(&effects), sel(&["a", "b"]));
        let effects = m.pointer_up((350.0, 50.0), &shapes);
        assert_eq!(ids(&effects), sel(&["a", "b"]));
        assert!(m.marquee_rect().is_none(), "release clears the marquee");
    }

    #[test]
    fn shift_marquee_adds_to_the_existing_selection() {
        let shapes = vec![
            note_at("kept", 0.0, 0.0, 10.0, 10.0, "U"),
            note_at("swept", 300.0, 300.0, 100.0, 100.0, "V"),
        ];
        let mut m = InteractionState::new();
        let effects = m.pointer_down(
            (250.0, 250.0),
            None,
            &sel(&["kept"]),
            &shapes,
            CanvasTool::Select,
            true,
        );
        assert!(effects.is_empty(), "shift-marquee must not clear first");
        let effects = m.pointer_move((350.0, 350.0), &shapes, 0.0);
        assert_eq!(ids(&effects), sel(&["kept", "swept"]));
    }

    // ---- move --------------------------------------------------------------

    #[test]
    fn a_move_previews_during_the_drag_and_commits_upserts_with_originals_as_undo() {
        let shapes = vec![note_at("a", 10.0, 10.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let _ = m.pointer_down(
            (20.0, 20.0),
            Some(&shapes[0]),
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            false,
        );
        let effects = m.pointer_move((50.0, 25.0), &shapes, 2.0);
        let Some(Effect::Preview(previewed)) = effects.first() else {
            panic!("expected a Preview, got {effects:?}");
        };
        assert_eq!(previewed[0].common().x, 40.0);
        assert_eq!(previewed[0].common().y, 15.0);

        let effects = m.pointer_up((120.0, 220.0), &shapes);
        let Some(Effect::Commit { redo, undo }) = effects.first() else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("move must commit upserts");
        };
        assert_eq!((shape.common().x, shape.common().y), (110.0, 210.0));
        let CanvasOp::UpsertShape { shape } = &undo[0] else {
            panic!("undo must upsert the original");
        };
        assert_eq!((shape.common().x, shape.common().y), (10.0, 10.0));
    }

    #[test]
    fn a_jitter_within_the_click_slop_neither_previews_nor_commits() {
        let shapes = vec![note_at("a", 10.0, 10.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let _ = m.pointer_down(
            (20.0, 20.0),
            Some(&shapes[0]),
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            false,
        );
        assert!(m.pointer_move((21.0, 19.0), &shapes, 3.0).is_empty());
        assert!(m.pointer_up((21.0, 19.0), &shapes).is_empty());
    }

    #[test]
    fn moving_an_arrow_carries_its_absolute_endpoints() {
        let arrow = Shape::Arrow {
            common: ShapeCommon {
                id: "ar".to_string(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 50.0,
                z: frac("U"),
                parent_id: None,
            },
            start: ArrowEnd {
                x: 0.0,
                y: 0.0,
                bind: None,
            },
            end: ArrowEnd {
                x: 100.0,
                y: 50.0,
                bind: None,
            },
            style: ShapeStyle::default(),
            label: String::new(),
        };
        let moved = translate_shape(&arrow, 7.0, -3.0);
        let Shape::Arrow {
            common, start, end, ..
        } = &moved
        else {
            panic!("still an arrow");
        };
        assert_eq!((common.x, common.y), (7.0, -3.0));
        assert_eq!((start.x, start.y), (7.0, -3.0));
        assert_eq!((end.x, end.y), (107.0, 47.0));
    }

    // ---- resize ------------------------------------------------------------

    #[test]
    fn each_of_the_eight_handles_moves_exactly_its_own_edges() {
        let origin = Bbox {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 60.0,
        };
        // (handle, drag target, expected box)
        let cases = [
            (
                Handle::Se,
                (140.0, 100.0),
                Bbox {
                    x: 10.0,
                    y: 20.0,
                    w: 130.0,
                    h: 80.0,
                },
            ),
            (
                Handle::Nw,
                (0.0, 0.0),
                Bbox {
                    x: 0.0,
                    y: 0.0,
                    w: 110.0,
                    h: 80.0,
                },
            ),
            (
                Handle::Ne,
                (150.0, 10.0),
                Bbox {
                    x: 10.0,
                    y: 10.0,
                    w: 140.0,
                    h: 70.0,
                },
            ),
            (
                Handle::Sw,
                (0.0, 90.0),
                Bbox {
                    x: 0.0,
                    y: 20.0,
                    w: 110.0,
                    h: 70.0,
                },
            ),
            (
                Handle::N,
                (999.0, 10.0),
                Bbox {
                    x: 10.0,
                    y: 10.0,
                    w: 100.0,
                    h: 70.0,
                },
            ),
            (
                Handle::S,
                (999.0, 100.0),
                Bbox {
                    x: 10.0,
                    y: 20.0,
                    w: 100.0,
                    h: 80.0,
                },
            ),
            (
                Handle::E,
                (150.0, 999.0),
                Bbox {
                    x: 10.0,
                    y: 20.0,
                    w: 140.0,
                    h: 60.0,
                },
            ),
            (
                Handle::W,
                (0.0, 999.0),
                Bbox {
                    x: 0.0,
                    y: 20.0,
                    w: 110.0,
                    h: 60.0,
                },
            ),
        ];
        for (handle, target, expected) in cases {
            assert_eq!(
                resize_bbox(origin, handle, target),
                expected,
                "handle {handle:?}"
            );
        }
    }

    #[test]
    fn dragging_a_handle_past_the_opposite_edge_flips_and_normalizes() {
        let origin = Bbox {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 60.0,
        };
        // Se dragged left-above the Nw corner: the box flips around it.
        let flipped = resize_bbox(origin, Handle::Se, (0.0, 0.0));
        assert_eq!(
            flipped,
            Bbox {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 20.0
            }
        );
        // Collapsing onto the anchor still leaves MIN_RESIZE.
        let collapsed = resize_bbox(origin, Handle::E, (10.0, 0.0));
        assert!((collapsed.w - MIN_RESIZE).abs() < f64::EPSILON);
    }

    #[test]
    fn resize_scales_positions_extents_ink_points_and_arrow_ends() {
        let ink = Shape::Ink {
            common: ShapeCommon {
                id: "ink".to_string(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                z: frac("U"),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            points: vec![[0.0, 0.0, 0.5], [100.0, 100.0, 0.5]],
        };
        let neighbor = note_at("n", 100.0, 0.0, 100.0, 100.0, "V");
        let from = Bbox {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let to = Bbox {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 200.0,
        };
        let scaled = scale_shapes(&[ink, neighbor], from, to);
        let Shape::Ink { common, points, .. } = &scaled[0] else {
            panic!("ink stays ink");
        };
        assert_eq!((common.w, common.h), (50.0, 200.0));
        assert_eq!(points[1], [50.0, 200.0, 0.5], "pressure must not scale");
        let n = scaled[1].common();
        assert_eq!((n.x, n.y, n.w, n.h), (50.0, 0.0, 50.0, 200.0));
    }

    #[test]
    fn a_resize_drag_commits_scaled_upserts_and_a_touch_without_movement_commits_nothing() {
        let shapes = vec![note_at("a", 0.0, 0.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let _ = m.begin_resize(Handle::Se, shapes.clone());
        let effects = m.pointer_move((200.0, 50.0), &shapes, 0.0);
        assert!(matches!(effects.first(), Some(Effect::Preview(_))));
        let effects = m.pointer_up((200.0, 50.0), &shapes);
        let Some(Effect::Commit { redo, undo }) = effects.first() else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("resize commits upserts");
        };
        assert_eq!((shape.common().w, shape.common().h), (200.0, 50.0));
        assert_eq!(undo.len(), 1);

        // A handle grabbed and released without any move event: no commit.
        let mut m = InteractionState::new();
        let _ = m.begin_resize(Handle::Se, shapes.clone());
        assert!(m.pointer_up((100.0, 100.0), &shapes).is_empty());
    }

    // ---- cancel ------------------------------------------------------------

    #[test]
    fn esc_rolls_a_move_back_to_the_snapshot_and_restores_a_shift_marquee_base() {
        let shapes = vec![note_at("a", 10.0, 10.0, 100.0, 100.0, "U")];
        let mut m = InteractionState::new();
        let _ = m.pointer_down(
            (20.0, 20.0),
            Some(&shapes[0]),
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            false,
        );
        let _ = m.pointer_move((500.0, 500.0), &shapes, 0.0);
        let effects = m.cancel();
        let Some(Effect::Preview(restored)) = effects.first() else {
            panic!("cancel must roll back via Preview, got {effects:?}");
        };
        assert_eq!(restored[0].common().x, 10.0, "back to the snapshot");
        assert!(!m.is_dragging());
        // A canceled drag must not commit on a later (stray) pointer up.
        assert!(m.pointer_up((500.0, 500.0), &shapes).is_empty());

        // Canceling a shift-marquee restores the pre-marquee selection.
        let _ = m.pointer_down(
            (0.0, 0.0),
            None,
            &sel(&["a"]),
            &shapes,
            CanvasTool::Select,
            true,
        );
        let effects = m.cancel();
        assert_eq!(ids(&effects), sel(&["a"]));
    }

    // ---- keyboard helpers --------------------------------------------------

    #[test]
    fn delete_and_nudge_ops_cover_exactly_the_selection() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 10.0, 10.0, "U"),
            note_at("b", 50.0, 50.0, 10.0, 10.0, "V"),
        ];
        assert_eq!(
            delete_ops(&sel(&["a", "b"])),
            vec![
                CanvasOp::DeleteShape { id: "a".into() },
                CanvasOp::DeleteShape { id: "b".into() },
            ]
        );
        let nudged = nudge_ops(&shapes, &sel(&["b"]), 0.0, -10.0);
        assert_eq!(nudged.len(), 1);
        let CanvasOp::UpsertShape { shape } = &nudged[0] else {
            panic!("nudge upserts");
        };
        assert_eq!((shape.common().x, shape.common().y), (50.0, 40.0));
    }

    #[test]
    fn duplicate_mints_new_ids_offsets_the_copies_and_stacks_them_on_top() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 10.0, 10.0, "U"),
            note_at("b", 50.0, 50.0, 10.0, 10.0, "V"),
        ];
        let mut counter = 0;
        let mut mint = move || {
            counter += 1;
            format!("dup{counter}")
        };
        let (ops, new_ids) = duplicate_ops(&shapes, &sel(&["a", "b"]), &mut mint);
        assert_eq!(new_ids, sel(&["dup1", "dup2"]));
        let top = frac("V");
        let mut prev = top.clone();
        for (op, original_x) in ops.iter().zip([0.0, 50.0]) {
            let CanvasOp::UpsertShape { shape } = op else {
                panic!("duplicate upserts");
            };
            let c = shape.common();
            assert_eq!(c.x, original_x + DUPLICATE_OFFSET);
            assert!(c.z > prev, "each copy stacks above the previous top");
            prev = c.z.clone();
        }
        assert!(prev > top);
    }

    #[test]
    fn selection_bbox_is_the_union_and_ignores_vanished_ids() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 10.0, 10.0, "U"),
            note_at("b", 90.0, 40.0, 10.0, 10.0, "V"),
        ];
        assert_eq!(
            selection_bbox(&shapes, &sel(&["a", "b", "ghost"])),
            Some(Bbox {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 50.0
            })
        );
        assert_eq!(selection_bbox(&shapes, &sel(&["ghost"])), None);
    }

    // ---- creation tools ----------------------------------------------------

    #[test]
    fn create_kind_maps_exactly_the_four_box_creation_tools() {
        use aleph_protocol::canvas::GeoForm;
        assert_eq!(
            create_kind(CanvasTool::Geo(GeoForm::Ellipse)),
            Some(CreateKind::Geo(GeoForm::Ellipse))
        );
        assert_eq!(create_kind(CanvasTool::Note), Some(CreateKind::Note));
        assert_eq!(create_kind(CanvasTool::Text), Some(CreateKind::Text));
        assert_eq!(create_kind(CanvasTool::Frame), Some(CreateKind::Frame));
        for inert in [
            CanvasTool::Select,
            CanvasTool::Pan,
            CanvasTool::Draw,
            CanvasTool::Arrow,
        ] {
            assert_eq!(create_kind(inert), None, "{inert:?} is not a box-create");
        }
    }

    #[test]
    fn a_reverse_creation_drag_normalizes_its_bbox_and_commits_with_delete_as_undo() {
        use aleph_protocol::canvas::GeoForm;
        let shapes = vec![note_at("a", 0.0, 0.0, 10.0, 10.0, "V")];
        let mut m = InteractionState::new();
        let effects = m.begin_create(
            CreateKind::Geo(GeoForm::Rect),
            (100.0, 100.0),
            "fresh".to_string(),
            &shapes,
        );
        assert_eq!(
            ids(&effects),
            Vec::<String>::new(),
            "starting a creation clears the selection"
        );
        assert!(m.is_dragging());

        // Drag up-left: origin becomes the bottom-right corner.
        let effects = m.pointer_move((20.0, 40.0), &shapes, 2.0);
        let Some(Effect::Preview(previewed)) = effects.first() else {
            panic!("expected a Preview, got {effects:?}");
        };
        let c = previewed[0].common();
        assert_eq!(
            (c.x, c.y, c.w, c.h),
            (20.0, 40.0, 80.0, 60.0),
            "a reverse drag must normalize to positive extents"
        );
        assert!(
            c.z > frac("V"),
            "the created shape must stack above the document top"
        );

        let effects = m.pointer_up((20.0, 40.0), &shapes);
        assert_eq!(ids(&effects), sel(&["fresh"]), "the new shape is selected");
        let Some(Effect::Commit { redo, undo }) =
            effects.iter().find(|e| matches!(e, Effect::Commit { .. }))
        else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("creation commits an upsert");
        };
        assert!(matches!(
            shape,
            Shape::Geo {
                form: GeoForm::Rect,
                ..
            }
        ));
        let c = shape.common();
        assert_eq!((c.x, c.y, c.w, c.h), (20.0, 40.0, 80.0, 60.0));
        assert_eq!(
            undo,
            &vec![CanvasOp::DeleteShape { id: "fresh".into() }],
            "undoing a creation deletes it"
        );
        assert!(!m.is_dragging());
    }

    #[test]
    fn a_click_with_a_creation_tool_lands_a_default_sized_shape_centered_on_the_press() {
        let mut m = InteractionState::new();
        let _ = m.begin_create(CreateKind::Note, (50.0, 60.0), "n".to_string(), &[]);
        // Jitter inside the slop, then release: still a click.
        assert!(m.pointer_move((51.0, 59.0), &[], 3.0).is_empty());
        let effects = m.pointer_up((51.0, 59.0), &[]);
        let Some(Effect::Commit { redo, .. }) =
            effects.iter().find(|e| matches!(e, Effect::Commit { .. }))
        else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("creation commits an upsert");
        };
        let c = shape.common();
        assert_eq!(
            (c.x, c.y, c.w, c.h),
            (-50.0, -40.0, 200.0, 200.0),
            "a click lands the default box centered on the press point"
        );
    }

    #[test]
    fn the_text_tool_hands_off_to_the_text_editor_instead_of_committing() {
        let mut m = InteractionState::new();
        let _ = m.begin_create(CreateKind::Text, (10.0, 10.0), "t1".to_string(), &[]);
        let effects = m.pointer_up((10.0, 10.0), &[]);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Commit { .. })),
            "a fresh text shape must not commit before its text exists"
        );
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Preview(_))),
            "the empty text shape must land locally for the overlay to sit on"
        );
        let Some(Effect::BeginTextEdit { shape, fresh }) = effects
            .iter()
            .find(|e| matches!(e, Effect::BeginTextEdit { .. }))
        else {
            panic!("expected BeginTextEdit, got {effects:?}");
        };
        assert!(matches!(shape, Shape::Text { .. }));
        assert_eq!(shape.id(), "t1");
        assert!(*fresh, "a tool-created text shape edits as fresh");
    }

    #[test]
    fn esc_discards_a_creation_preview_without_committing() {
        let mut m = InteractionState::new();
        let _ = m.begin_create(CreateKind::Frame, (0.0, 0.0), "f1".to_string(), &[]);
        let _ = m.pointer_move((300.0, 200.0), &[], 0.0);
        let effects = m.cancel();
        assert_eq!(
            effects,
            vec![Effect::DiscardPreview(vec!["f1".to_string()])],
            "cancel must delete the previewed shape, not restore it"
        );
        assert!(!m.is_dragging());
        // A canceled creation must not commit on a stray pointer up…
        assert!(m.pointer_up((300.0, 200.0), &[]).is_empty());

        // …and canceling before any movement has nothing to discard.
        let mut m = InteractionState::new();
        let _ = m.begin_create(CreateKind::Note, (0.0, 0.0), "n1".to_string(), &[]);
        assert!(m.cancel().is_empty());
    }

    // ---- draw tool ---------------------------------------------------------

    #[test]
    fn draw_decimates_close_points_and_normalizes_the_stroke_to_its_bbox() {
        let mut m = InteractionState::new();
        let effects = m.begin_draw((10.0, 20.0), 0.5, "ink1".to_string(), &[], 2.0);
        assert_eq!(
            ids(&effects),
            Vec::<String>::new(),
            "starting a stroke clears the selection"
        );
        let Some(Effect::Preview(previewed)) =
            effects.iter().find(|e| matches!(e, Effect::Preview(_)))
        else {
            panic!("the press must preview a dot, got {effects:?}");
        };
        let Shape::Ink { points, .. } = &previewed[0] else {
            panic!("draw previews ink");
        };
        assert_eq!(points.len(), 1);

        // Within min_dist (2.0 world units): decimated, no new preview.
        let effects = m.draw_move((11.0, 20.5), 0.5).expect("drawing is active");
        assert!(effects.is_empty(), "a sub-threshold move adds no point");

        // Beyond: the point lands and the preview re-normalizes.
        let effects = m.draw_move((50.0, 60.0), 0.9).expect("drawing is active");
        let Some(Effect::Preview(previewed)) = effects.first() else {
            panic!("expected a Preview, got {effects:?}");
        };
        let Shape::Ink { common, points, .. } = &previewed[0] else {
            panic!("draw previews ink");
        };
        assert_eq!(
            (common.x, common.y, common.w, common.h),
            (10.0, 20.0, 40.0, 40.0),
            "the bbox is the points' extent"
        );
        assert_eq!(
            points,
            &vec![[0.0, 0.0, 0.5], [40.0, 40.0, 0.9]],
            "stored points are origin-relative and keep their pressure"
        );

        // Release commits with delete as undo, and no selection change —
        // the tool stays armed for the next stroke.
        let effects = m.pointer_up((50.0, 60.0), &[]);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::SetSelection(_))),
            "a finished stroke must not grab the selection"
        );
        let Some(Effect::Commit { redo, undo }) = effects.first() else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("draw commits an upsert");
        };
        assert!(matches!(shape, Shape::Ink { .. }));
        assert_eq!(
            undo,
            &vec![CanvasOp::DeleteShape { id: "ink1".into() }],
            "undoing a stroke deletes it"
        );
        assert!(!m.is_dragging());
    }

    #[test]
    fn draw_move_answers_none_outside_an_ink_drag_and_esc_discards_the_stroke() {
        let mut m = InteractionState::new();
        assert!(m.draw_move((0.0, 0.0), 0.5).is_none(), "no stroke active");

        let _ = m.begin_draw((0.0, 0.0), 0.5, "ink1".to_string(), &[], 2.0);
        let effects = m.cancel();
        assert_eq!(
            effects,
            vec![Effect::DiscardPreview(vec!["ink1".to_string()])],
            "even an unmoved stroke previewed its dot — cancel must discard it"
        );
        assert!(!m.is_dragging());
    }

    #[test]
    fn effective_pressure_normalizes_a_pressureless_reading_to_the_spec_default() {
        assert!((effective_pressure(0.0) - 0.5).abs() < f32::EPSILON);
        assert!((effective_pressure(0.8) - 0.8).abs() < f32::EPSILON);
    }

    // ---- arrow tool --------------------------------------------------------

    #[test]
    fn an_arrow_drag_binds_both_endpoints_and_commits_with_delete_as_undo() {
        let shapes = vec![
            note_at("a", 0.0, 0.0, 100.0, 100.0, "U"),
            note_at("b", 300.0, 0.0, 100.0, 100.0, "V"),
        ];
        let mut m = InteractionState::new();
        let effects = m.begin_arrow((50.0, 50.0), Some(&shapes[0]), "ar1".to_string(), &shapes);
        assert_eq!(ids(&effects), Vec::<String>::new());

        // Over empty canvas: preview, no hover target.
        let effects = m.pointer_move((200.0, 50.0), &shapes, 2.0);
        let Some(Effect::Preview(previewed)) = effects.first() else {
            panic!("expected a Preview, got {effects:?}");
        };
        let Shape::Arrow { start, end, .. } = &previewed[0] else {
            panic!("arrow previews an arrow");
        };
        assert_eq!(
            start.bind.as_deref(),
            Some("a"),
            "the press bound the start"
        );
        assert!(end.bind.is_none(), "the end binds at release, not mid-drag");
        assert_eq!(m.arrow_hover(), None);

        // Over shape b: the would-bind target surfaces for highlighting. The
        // doc now also holds the previewed draft itself — it must never be
        // its own bind target.
        let mut with_draft = shapes.clone();
        with_draft.push(previewed[0].clone());
        let _ = m.pointer_move((350.0, 50.0), &with_draft, 2.0);
        assert_eq!(m.arrow_hover(), Some("b".to_string()));

        let effects = m.pointer_up((350.0, 50.0), &with_draft);
        assert_eq!(ids(&effects), sel(&["ar1"]), "the new arrow is selected");
        let Some(Effect::Commit { redo, undo }) =
            effects.iter().find(|e| matches!(e, Effect::Commit { .. }))
        else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("arrow commits an upsert");
        };
        let Shape::Arrow {
            common, start, end, ..
        } = shape
        else {
            panic!("still an arrow");
        };
        assert_eq!((start.x, start.y), (50.0, 50.0));
        assert_eq!(start.bind.as_deref(), Some("a"));
        assert_eq!((end.x, end.y), (350.0, 50.0));
        assert_eq!(end.bind.as_deref(), Some("b"), "the release bound the end");
        assert_eq!(
            (common.x, common.y, common.w, common.h),
            (50.0, 50.0, 300.0, 0.0),
            "the bbox spans the two endpoints"
        );
        assert_eq!(undo, &vec![CanvasOp::DeleteShape { id: "ar1".into() }]);
        assert_eq!(m.arrow_hover(), None, "the drag is over");
    }

    #[test]
    fn an_arrow_click_commits_nothing_and_esc_discards_a_moved_draft() {
        let mut m = InteractionState::new();
        let _ = m.begin_arrow((10.0, 10.0), None, "ar1".to_string(), &[]);
        assert!(
            m.pointer_up((10.0, 10.0), &[]).is_empty(),
            "an arrow needs a drag — a click creates nothing"
        );

        let _ = m.begin_arrow((10.0, 10.0), None, "ar2".to_string(), &[]);
        let _ = m.pointer_move((200.0, 200.0), &[], 2.0);
        assert_eq!(
            m.cancel(),
            vec![Effect::DiscardPreview(vec!["ar2".to_string()])]
        );
        // …but an unmoved draft previewed nothing, so there is nothing to
        // discard.
        let _ = m.begin_arrow((10.0, 10.0), None, "ar3".to_string(), &[]);
        assert!(m.cancel().is_empty());
    }

    #[test]
    fn a_zero_extent_creation_drag_is_clamped_to_a_visible_shape() {
        let mut m = InteractionState::new();
        let _ = m.begin_create(CreateKind::Note, (0.0, 0.0), "n1".to_string(), &[]);
        // A perfectly horizontal drag: height would be zero.
        let _ = m.pointer_move((50.0, 0.0), &[], 2.0);
        let effects = m.pointer_up((50.0, 0.0), &[]);
        let Some(Effect::Commit { redo, .. }) =
            effects.iter().find(|e| matches!(e, Effect::Commit { .. }))
        else {
            panic!("expected a Commit, got {effects:?}");
        };
        let CanvasOp::UpsertShape { shape } = &redo[0] else {
            panic!("creation commits an upsert");
        };
        assert_eq!(shape.common().w, 50.0);
        assert!((shape.common().h - MIN_RESIZE).abs() < f64::EPSILON);
    }
}

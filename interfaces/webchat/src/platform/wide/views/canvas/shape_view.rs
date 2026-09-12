//! Read-only SVG rendering of one whiteboard shape — every [`Shape`] variant
//! gets a visual here, all of it inside the editor's world-transform group.
//!
//! # Reactivity shape
//!
//! [`ShapeView`] takes a `Memo<Option<Shape>>` (minted per-id by the editor's
//! keyed `<For>`): the DOM node identity is keyed by shape id, while content
//! changes re-render through the memo. `None` (the shape vanished from the
//! doc while its row is still being reconciled) renders nothing.
//!
//! # Colors are theme tokens, resolved in `style=`
//!
//! `ShapeStyle.color` is a named palette slot or a `#rrggbb` literal on the
//! wire; [`palette_var`] resolves a slot to a `var(--color-*)` reference and
//! passes a literal through. Resolution must land in the `style` attribute,
//! not in `fill=`/`stroke=` presentation attributes — CSS custom properties
//! do not resolve inside bare SVG attributes, and a literal `var(…)` there
//! paints black in every browser.
//!
//! # Drawing infrastructure (2026-09-12) — what this build renders
//!
//! - Every `Geo` form is a `<path>` from `geo_path::geo_cmds` (no native
//!   `<rect>`/`<ellipse>`/`<polygon>` for geometry — the `<rect>`s that
//!   remain below are the Note card, the Frame, the Image placeholder, the
//!   HTML placeholder and the AI-frame chrome, none of which is a `GeoForm`);
//!   `Path` is the contract-parsed `d` re-emitted.
//! - `StrokeKind::Dashed` / `Dotted` are `stroke-dasharray`; `Sketch` is the
//!   seeded hand-drawn synthesis (`sketch.rs`, seeded by the shape id) on
//!   every Geo / Path / Arrow outline.
//! - `Arrow.bend` is the three-point arc of `arrow_geom.rs`; heads follow
//!   `head_start` / `head_end` and lean along the arc's tangent.
//! - `reveal` wraps the shape in the `reveal.rs` classes (and the outline
//!   gets `pathLength="1"` for the draw-on mode) — attributes that exist
//!   ONLY when a reveal does, so an un-animated document's markup is
//!   unchanged. Playback itself is the surfaces' `PlaybackButton`.
//!
//! # Ink and arrows (Task 15)
//!
//! - `Ink` renders the pressure-aware freehand outline (`freehand.rs`) as a
//!   single filled path — the polygon is the stroke's silhouette.
//! - `Arrow` endpoints follow their bound shapes live:
//!   `arrow_geom::resolve_arrow_ends` reads the bound shapes out of the
//!   document and clips each endpoint onto the bound shape's outline, aimed
//!   at the other end. The resolution sits behind a `Memo` so an unbound
//!   arrow never subscribes to the doc signal at all.
//!
//! # HTML frames (Task 16)
//!
//! `Shape::Html` renders in TWO layers. The SVG half here stays the labelled
//! placeholder box — it is what shows while the srcdoc is in flight (and all
//! that shows if the fetch fails). The live half is [`HtmlFrameOverlay`],
//! mounted by the editor inside its world-transformed HTML overlay: one
//! sandboxed iframe per `Html` shape, `sandbox="allow-scripts"` and NEVER
//! `allow-same-origin` — model-authored HTML runs in an opaque origin that
//! cannot reach the Panel's RPCs, storage or cookies. A source-level census
//! below pins both halves of that sentence.
//!
//! srcdoc content arrives over `canvas.asset.get` (base64 → text), not over
//! the capability byte route: the route serves `text/html` as `text/plain`
//! by design (the server-side XSS boundary), so the RPC is the only path
//! that yields usable source. Fetches are dedup'd by
//! [`super::asset_ingest::SrcdocCache`].
//!
//! # What is deliberately simple in this task
//!
//! - Text does not wrap — explicit `\n` breaks only, the `plan_dag.rs`
//!   limitation. The text-editing overlay (Task 14) owns real layout.

use aleph_protocol::canvas::{
    cmds_to_d, is_hex_color, AiFrameStatus, GeoForm, PathCmd, Shape, ShapeCommon, ShapeStyle,
    SizeKind, StrokeKind,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::arrow_geom::{self, HeadMark};
use super::asset_ingest::SrcdocCache;
use super::interaction::Bbox;
use super::{freehand, geo_path, reveal, sketch};
use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, I18nCtx};
use crate::state::canvas::CanvasState;

/// Stroke width of every Geo / Path / Arrow outline, world units — also
/// what scales the sketch synthesis. `pub(super)`: the export draws (and
/// sketches) at the same width.
pub(super) const OUTLINE_STROKE_W: f64 = 2.0;

/// The `d` an outline renders with: the clean command list, or its seeded
/// sketch for [`StrokeKind::Sketch`]. `seed` is the shape id (the wire
/// contract's determinism promise). `pub(super)`: the export serializer
/// makes the same choice through the same function.
#[must_use]
pub(super) fn outline_d(cmds: &[PathCmd], stroke: StrokeKind, seed: &str) -> String {
    if stroke == StrokeKind::Sketch {
        sketch::sketch_d(cmds, seed, OUTLINE_STROKE_W)
    } else {
        cmds_to_d(cmds)
    }
}

/// Reveal hooks for one shape: `(outline class, pathLength, body class)`,
/// all `None` when the shape has no reveal — so nothing about an
/// un-animated shape's markup changes. `pub(super)`: the export emits the
/// same hooks.
#[must_use]
pub(super) fn reveal_hooks(
    common: &ShapeCommon,
) -> (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
) {
    match &common.reveal {
        None => (None, None, None),
        Some(r) => (
            Some(reveal::OUTLINE_CLASS),
            reveal::needs_path_length(Some(r)).then_some("1"),
            Some(reveal::BODY_CLASS),
        ),
    }
}

/// One shape, looked up by id from the editor's shape map.
#[component]
pub(super) fn ShapeView(shape: Memo<Option<Shape>>) -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();
    move || shape.get().map(|s| shape_svg(&s, canvas, i18n))
}

/// Capability URL for one asset's bytes.
///
/// The server's byte route is `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`
/// and `canvas.get` mints `asset_base` = `/canvas-asset/<cap>/<canvas_id>`
/// (pinned by the handler test `get_mints_an_asset_base_bound_to_the_canvas`)
/// — so the href is base + one path segment, nothing else.
#[must_use]
fn asset_href(asset_base: &str, asset_id: &str) -> String {
    format!("{asset_base}/{asset_id}")
}

/// Resolve a wire color to CSS: a palette slot becomes a theme token
/// reference, a `#rrggbb` literal (the contract's `check_color` shape) is
/// used as-is.
///
/// Unknown slots (including the empty default) resolve to the neutral ink —
/// an unrecognized color must degrade to *visible*, never to an error.
/// `pub(super)`: the toolbar's swatches paint each slot with the same
/// token the shapes will be drawn in.
#[must_use]
pub(super) fn palette_var(slot: &str) -> String {
    match slot {
        "red" => "var(--color-danger)",
        "orange" => "var(--color-warning)",
        "yellow" => "var(--color-chart-3)",
        "green" => "var(--color-success)",
        "blue" => "var(--color-info)",
        "violet" => "var(--color-primary)",
        hex if is_hex_color(hex) => hex,
        _ => "var(--color-text-secondary)",
    }
    .to_string()
}

/// `stroke-dasharray` for a stroke kind; `None` draws a continuous line.
/// `Sketch` is `None` here because its look comes from the path data
/// ([`outline_d`]), not from a dash pattern. `pub(super)`: the export
/// serializer dashes the same way.
#[must_use]
pub(super) fn stroke_dasharray(stroke: StrokeKind) -> Option<&'static str> {
    match stroke {
        StrokeKind::Solid | StrokeKind::Sketch => None,
        StrokeKind::Dashed => Some("8 6"),
        StrokeKind::Dotted => Some("2 5"),
    }
}

/// Fill for a closed shape: a translucent wash of its stroke color, or none.
#[must_use]
fn fill_css(style: &ShapeStyle) -> String {
    if style.fill {
        format!(
            "color-mix(in oklch, {} 18%, transparent)",
            palette_var(&style.color)
        )
    } else {
        "none".to_string()
    }
}

/// Body-text color: colored shapes write in their color, the default slot in
/// the primary text token (secondary is too faint for body text).
/// `pub(super)`: the text-editing overlay writes in the same ink so the
/// textarea and the committed SVG text cannot disagree.
#[must_use]
pub(super) fn text_fill(style: &ShapeStyle) -> String {
    match style.color.as_str() {
        "red" | "orange" | "yellow" | "green" | "blue" | "violet" => palette_var(&style.color),
        hex if is_hex_color(hex) => hex.to_string(),
        _ => "var(--color-text-primary)".to_string(),
    }
}

/// `pub(super)`: shared with the text-editing overlay (same reasoning as
/// [`text_fill`] — one source for how big a shape's text renders).
#[must_use]
pub(super) fn font_size_for(size: SizeKind) -> f64 {
    match size {
        SizeKind::Small => 12.0,
        SizeKind::Medium => 16.0,
        SizeKind::Large => 24.0,
    }
}

#[must_use]
fn ink_stroke_width(size: SizeKind) -> f64 {
    match size {
        SizeKind::Small => 2.0,
        SizeKind::Medium => 3.5,
        SizeKind::Large => 6.0,
    }
}

/// The freehand base diameter for a stroke size — 2× the old polyline
/// stroke-width, because at the resting pressure of 0.5 the outline's
/// half-width is a quarter of the base size ([`freehand::THINNING`] math),
/// which keeps the on-screen weight of existing strokes unchanged.
/// `pub(super)`: the export serializer draws the same silhouette.
#[must_use]
pub(super) fn freehand_size(size: SizeKind) -> f64 {
    ink_stroke_width(size) * 2.0
}

/// First `max` chars of an AI prompt, `…`-terminated — char-boundary safe
/// (a CJK prompt sliced by bytes would panic).
/// `pub(super)`: the export serializer excerpts the same way.
#[must_use]
pub(super) fn prompt_excerpt(prompt: &str, max: usize) -> String {
    if prompt.chars().count() <= max {
        return prompt.to_string();
    }
    let mut s: String = prompt.chars().take(max).collect();
    s.push('…');
    s
}

/// `\n`-split text as a stack of `<text>` lines, first baseline at `y`.
fn text_block(x: f64, y: f64, text: &str, fs: f64, fill: &str) -> AnyView {
    let line_height = fs * 1.4;
    text.split('\n')
        .enumerate()
        .map(|(i, line)| {
            let line = line.to_string();
            view! {
                <text
                    x=x
                    y=y + line_height * (i as f64)
                    font-size=fs
                    style=format!("fill: {fill}; user-select: none;")
                >
                    {line}
                </text>
            }
        })
        .collect_view()
        .into_any()
}

/// Wrap a shape's markup in its reveal group when it has a reveal — the
/// selector root `reveal_css` targets. A shape without a reveal is returned
/// untouched.
fn reveal_group(common: &ShapeCommon, inner: AnyView) -> AnyView {
    if common.reveal.is_none() {
        return inner;
    }
    let class = reveal::shape_class(&common.id);
    view! { <g class=class>{inner}</g> }.into_any()
}

/// The reveal group around markup that has no outline of its own (text,
/// cards, images): the whole shape is "body", fading in during the second
/// phase of a draw-on reveal.
fn reveal_body(common: &ShapeCommon, inner: AnyView) -> AnyView {
    let (_, _, body) = reveal_hooks(common);
    let inner = match body {
        None => inner,
        Some(class) => view! { <g class=class>{inner}</g> }.into_any(),
    };
    reveal_group(common, inner)
}

fn shape_svg(shape: &Shape, canvas: CanvasState, i18n: I18nCtx) -> AnyView {
    match shape {
        Shape::Geo {
            common,
            form,
            style,
            text,
        } => reveal_group(common, geo_svg(common, *form, style, text)),
        Shape::Ink {
            common,
            style,
            points,
        } => reveal_body(common, ink_svg(common, style, points)),
        Shape::Text {
            common,
            style,
            text,
        } => {
            let fs = font_size_for(style.size);
            reveal_body(
                common,
                view! {
                    <g>{text_block(common.x, common.y + fs, text, fs, &text_fill(style))}</g>
                }
                .into_any(),
            )
        }
        Shape::Note {
            common,
            style,
            text,
        } => reveal_body(common, note_svg(common, style, text)),
        Shape::Image {
            common, asset_id, ..
        } => reveal_body(common, image_svg(common, asset_id, canvas)),
        Shape::Frame { common, title, .. } => reveal_body(common, frame_svg(common, title)),
        Shape::Html { common, .. } => reveal_body(common, html_placeholder_svg(common, i18n)),
        Shape::Arrow { common, .. } => reveal_group(common, arrow_svg(shape, canvas)),
        Shape::Path {
            common,
            style,
            d,
            closed,
        } => reveal_group(common, path_svg(common, style, d, *closed)),
        Shape::AiImageFrame {
            common,
            prompt,
            status,
            ..
        } => reveal_body(common, ai_frame_svg(common, prompt, *status, i18n)),
    }
}

fn geo_svg(common: &ShapeCommon, form: GeoForm, style: &ShapeStyle, text: &str) -> AnyView {
    let stroke = palette_var(&style.color);
    let paint = format!("stroke: {stroke}; fill: {};", fill_css(style));
    let fs = font_size_for(style.size);
    let (outline_class, path_length, body_class) = reveal_hooks(common);
    let d = outline_d(
        &geo_path::geo_cmds(form, common.w, common.h),
        style.stroke,
        &common.id,
    );
    let label = (!text.is_empty()).then(|| {
        let text = text_block(
            common.x + 10.0,
            common.y + fs + 8.0,
            text,
            fs,
            &text_fill(style),
        );
        view! { <g class=body_class>{text}</g> }
    });
    view! {
        <g>
            <path
                d=d
                transform=format!("translate({} {})", common.x, common.y)
                class=outline_class
                pathLength=path_length
                style=paint
                stroke-width=OUTLINE_STROKE_W
                stroke-linejoin="round"
                stroke-linecap="round"
                stroke-dasharray=stroke_dasharray(style.stroke)
            />
            {label}
        </g>
    }
    .into_any()
}

fn ink_svg(common: &ShapeCommon, style: &ShapeStyle, points: &[[f32; 3]]) -> AnyView {
    // The pressure-aware silhouette, filled — not a stroked polyline: the
    // outline's varying width IS the pressure rendering (`freehand.rs`).
    let d = freehand::outline_path_d(points, freehand_size(style.size));
    if d.is_empty() {
        return ().into_any();
    }
    view! {
        <g transform=format!("translate({} {})", common.x, common.y)>
            <path d=d style=format!("fill: {};", palette_var(&style.color)) />
        </g>
    }
    .into_any()
}

/// A `Shape::Path`: the contract-parsed `d` (sketched when the stroke asks
/// for it), translated to the shape origin (shape-local coordinates, the
/// `Ink` convention). A `d` this build cannot parse renders nothing — never
/// a guess.
fn path_svg(common: &ShapeCommon, style: &ShapeStyle, d: &str, closed: bool) -> AnyView {
    let Some(cmds) = geo_path::path_cmds(d, closed) else {
        return ().into_any();
    };
    let fill = if closed {
        fill_css(style)
    } else {
        "none".to_string()
    };
    let paint = format!("stroke: {}; fill: {fill};", palette_var(&style.color));
    let (outline_class, path_length, _) = reveal_hooks(common);
    view! {
        <g transform=format!("translate({} {})", common.x, common.y)>
            <path
                d=outline_d(&cmds, style.stroke, &common.id)
                class=outline_class
                pathLength=path_length
                style=paint
                stroke-width=OUTLINE_STROKE_W
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-dasharray=stroke_dasharray(style.stroke)
            />
        </g>
    }
    .into_any()
}

fn note_svg(common: &ShapeCommon, style: &ShapeStyle, text: &str) -> AnyView {
    // A note is always a filled card. The default slot reads as the classic
    // sticky yellow; named slots wash the card in their own color.
    let base = match style.color.as_str() {
        "" | "default" => "var(--color-warning)".to_string(),
        _ => palette_var(&style.color),
    };
    let fs = font_size_for(style.size);
    view! {
        <g>
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                rx=8
                style=format!(
                    "fill: color-mix(in oklch, {base} 22%, var(--color-surface-raised)); \
                     stroke: color-mix(in oklch, {base} 45%, transparent);"
                )
                stroke-width=1
            />
            {(!text.is_empty())
                .then(|| text_block(
                    common.x + 12.0,
                    common.y + fs + 10.0,
                    text,
                    fs,
                    "var(--color-text-primary)",
                ))}
        </g>
    }
    .into_any()
}

fn image_svg(common: &ShapeCommon, asset_id: &str, canvas: CanvasState) -> AnyView {
    // Read inside the ShapeView render closure: a refetched asset_base
    // (fresh capability) re-renders the image instead of leaving it pointed
    // at an expired URL.
    match canvas.asset_base.get() {
        Some(base) => {
            let href = asset_href(&base, asset_id);
            view! {
                <image
                    x=common.x
                    y=common.y
                    width=common.w
                    height=common.h
                    href=href
                    preserveAspectRatio="xMidYMid meet"
                />
            }
            .into_any()
        }
        // No capability in hand (stale doc, base still in flight): an outline
        // where the image will be — never a broken-image glyph.
        None => view! {
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                rx=4
                style="stroke: var(--color-border-strong); fill: var(--color-surface-sunken);"
                stroke-width=1
                stroke-dasharray="6 4"
            />
        }
        .into_any(),
    }
}

fn frame_svg(common: &ShapeCommon, title: &str) -> AnyView {
    view! {
        <g>
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                style="stroke: var(--color-border-strong); fill: var(--color-surface-raised);"
                stroke-width=2
            />
            {(!title.is_empty()).then(|| {
                let title = title.to_string();
                view! {
                    <text
                        x=common.x
                        y=common.y - 8.0
                        font-size=12
                        style="fill: var(--color-text-secondary); user-select: none;"
                    >
                        {title}
                    </text>
                }
            })}
        </g>
    }
    .into_any()
}

fn html_placeholder_svg(common: &ShapeCommon, i18n: I18nCtx) -> AnyView {
    view! {
        <g>
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                rx=6
                style="stroke: var(--color-info); \
                       fill: color-mix(in oklch, var(--color-info) 8%, transparent);"
                stroke-width=2
                stroke-dasharray="6 4"
            />
            <text
                x=common.x + common.w / 2.0
                y=common.y + common.h / 2.0
                text-anchor="middle"
                font-size=13
                style="fill: var(--color-text-tertiary); user-select: none;"
            >
                {t!(i18n, canvas.html_frame)}
            </text>
        </g>
    }
    .into_any()
}

fn head_mark_svg(mark: Option<HeadMark>, stroke: &str) -> AnyView {
    match mark {
        None => ().into_any(),
        Some(HeadMark::Polygon { points, filled }) => {
            let fill = if filled { stroke } else { "none" };
            view! {
                <polygon
                    points=points
                    style=format!("fill: {fill}; stroke: {stroke};")
                    stroke-width=OUTLINE_STROKE_W
                    stroke-linejoin="round"
                />
            }
            .into_any()
        }
        Some(HeadMark::Circle { cx, cy, r }) => view! {
            <circle cx=cx cy=cy r=r style=format!("fill: {stroke};") />
        }
        .into_any(),
        Some(HeadMark::Line { x1, y1, x2, y2 }) => view! {
            <line x1=x1 y1=y1 x2=x2 y2=y2 style=format!("stroke: {stroke};") stroke-width=OUTLINE_STROKE_W />
        }
        .into_any(),
    }
}

fn arrow_svg(shape: &Shape, canvas: CanvasState) -> AnyView {
    let Shape::Arrow {
        common,
        start,
        end,
        style,
        label,
        bend,
        head_start,
        head_end,
    } = shape
    else {
        return ().into_any();
    };
    let bend = *bend;
    let stroke = palette_var(&style.color);
    let dash = stroke_dasharray(style.stroke);
    let (outline_class, path_length, body_class) = reveal_hooks(common);
    let stroke_kind = style.stroke;
    let seed = common.id.clone();
    // Bound endpoints re-resolve whenever the document changes (the bound
    // shape may have moved), so the resolution reads the doc signal — but
    // only when a binding exists: an unbound arrow must not re-render on
    // every unrelated edit. The Memo dedupes by value, so doc churn that
    // leaves the anchors unchanged updates nothing downstream.
    let has_binds = start.bind.is_some() || end.bind.is_some();
    let (start, end) = (start.clone(), end.clone());
    let raw = ((start.x, start.y), (end.x, end.y));
    let shaft: Memo<arrow_geom::Shaft> = Memo::new(move |_| {
        let (a, b) = if has_binds {
            canvas.doc.with(|d| {
                let shapes = d.as_ref().map_or(&[][..], |d| d.shapes.as_slice());
                arrow_geom::resolve_arrow_ends(shapes, &start, &end)
            })
        } else {
            raw
        };
        arrow_geom::arrow_shaft(a, b, bend)
    });
    let label = label.to_string();
    let (head_start, head_end) = (*head_start, *head_end);
    let stroke_for_heads = stroke.clone();
    let stroke_for_label = stroke.clone();
    view! {
        <g>
            <path
                d=move || shaft.with(|s| outline_d(&s.cmds, stroke_kind, &seed))
                class=outline_class
                pathLength=path_length
                style=format!("stroke: {stroke}; fill: none;")
                stroke-width=OUTLINE_STROKE_W
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-dasharray=dash
            />
            <g class=body_class>
                {move || {
                    let s = shaft.get();
                    let stroke = stroke_for_heads.clone();
                    view! {
                        {head_mark_svg(arrow_geom::arrow_head_mark(head_end, s.end_from, s.end), &stroke)}
                        {head_mark_svg(arrow_geom::arrow_head_mark(head_start, s.start_from, s.start), &stroke)}
                    }
                }}
                {(!label.is_empty()).then(|| {
                    view! {
                        <text
                            x=move || shaft.with(|s| s.mid.0)
                            y=move || shaft.with(|s| s.mid.1 - 6.0)
                            text-anchor="middle"
                            font-size=12
                            style=format!("fill: {stroke_for_label}; user-select: none;")
                        >
                            {label}
                        </text>
                    }
                })}
            </g>
        </g>
    }
    .into_any()
}

fn ai_frame_svg(
    common: &ShapeCommon,
    prompt: &str,
    status: AiFrameStatus,
    i18n: I18nCtx,
) -> AnyView {
    let badge = match status {
        AiFrameStatus::Draft => "var(--color-info)",
        AiFrameStatus::Pending => "var(--color-warning)",
        AiFrameStatus::Done => "var(--color-success)",
        AiFrameStatus::Failed => "var(--color-danger)",
    };
    let status_label = match status {
        AiFrameStatus::Draft => t!(i18n, canvas.ai_status_draft).into_any(),
        AiFrameStatus::Pending => t!(i18n, canvas.ai_status_pending).into_any(),
        AiFrameStatus::Done => t!(i18n, canvas.ai_status_done).into_any(),
        AiFrameStatus::Failed => t!(i18n, canvas.ai_status_failed).into_any(),
    };
    view! {
        <g>
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                rx=6
                style="stroke: var(--color-primary); \
                       fill: color-mix(in oklch, var(--color-primary) 6%, transparent);"
                stroke-width=2
                stroke-dasharray="8 5"
            />
            <text
                x=common.x + 12.0
                y=common.y + 24.0
                font-size=13
                style="fill: var(--color-text-secondary); user-select: none;"
            >
                {prompt_excerpt(prompt, 80)}
            </text>
            <g>
                <rect
                    x=common.x + common.w - 88.0
                    y=common.y + 10.0
                    width=78
                    height=22
                    rx=11
                    style=format!(
                        "fill: color-mix(in oklch, {badge} 15%, var(--color-surface-raised)); \
                         stroke: {badge};"
                    )
                    stroke-width=1
                />
                <text
                    x=common.x + common.w - 49.0
                    y=common.y + 25.0
                    text-anchor="middle"
                    font-size=11
                    style=format!("fill: {badge}; user-select: none;")
                >
                    {status_label}
                </text>
            </g>
        </g>
    }
    .into_any()
}

/// The live half of `Shape::Html`: one sandboxed iframe per shape, mounted
/// in the editor's world-transformed HTML overlay (module doc).
///
/// # Mount/update split (why the closures are shaped this way)
///
/// A remounted iframe re-parses its srcdoc and reruns its scripts, so
/// nothing that changes often may cause a remount:
///
/// - the wrapper `<div>`'s geometry is a reactive *style* (a drag moves the
///   frame without touching the iframe),
/// - selection toggles only the iframe's `pointer-events` style,
/// - the iframe itself mounts once per resolved srcdoc — asset ids are
///   content-addressed, so the memo's value can only ever change None→Some.
///
/// # Pointer events
///
/// `pointer-events: none` by default — canvas gestures over the frame land
/// on the input plane below, so the frame can be marquee'd, moved and drawn
/// over like any shape. `auto` only while the shape is selected: that is the
/// explicit "I want to interact with this content" state. (While selected,
/// the iframe does eat gestures over its bbox — click empty canvas to
/// deselect and the frame goes inert again.)
#[component]
pub(super) fn HtmlFrameOverlay() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    // Fetch-dedup cache, keyed by asset id (asset_ingest.rs module doc).
    // StoredValue is not reactive, so inserts bump `cache_epoch` — the one
    // signal the srcdoc memos subscribe to.
    let cache: StoredValue<SrcdocCache> = StoredValue::new(SrcdocCache::new());
    let cache_epoch: RwSignal<u32> = RwSignal::new(0);

    // (canvas_id, [(shape_id, asset_id)]) — the render list and the fetch
    // driver share one memo, so they cannot disagree about what exists.
    let html_assets = Memo::new(move |_| {
        canvas.doc.with(|d| {
            d.as_ref().map(|d| {
                (
                    d.id.clone(),
                    d.shapes
                        .iter()
                        .filter_map(|s| match s {
                            Shape::Html { common, asset_id } => {
                                Some((common.id.clone(), asset_id.clone()))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                )
            })
        })
    });

    // Fetch driver: claim → `canvas.asset.get` → insert/abandon. The RPC —
    // not the capability byte route — because the route serves text/html as
    // text/plain by design (module doc).
    Effect::new(move |_| {
        let Some((canvas_id, entries)) = html_assets.get() else {
            return;
        };
        for (_shape_id, asset_id) in entries {
            let claimed = cache
                .try_update_value(|c| c.begin_fetch(&asset_id))
                .unwrap_or(false);
            if !claimed {
                continue;
            }
            let canvas_id = canvas_id.clone();
            spawn_local(async move {
                let fetched = CanvasApi::asset_get(&state, &canvas_id, &asset_id).await;
                match fetched {
                    Ok(asset) if asset.mime_type == "text/html" => {
                        let text = crate::views::voice::audio::base64_to_bytes(&asset.data)
                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                            .unwrap_or_default();
                        let _ = cache.try_update_value(|c| c.insert(&asset_id, text));
                        let _ = cache_epoch.try_update(|v| *v = v.wrapping_add(1));
                    }
                    Ok(asset) => {
                        // A non-html asset in an Html shape is a document
                        // bug (model-authored), not a user-actionable error:
                        // say so out loud, keep the placeholder.
                        let _ = cache.try_update_value(|c| c.abandon(&asset_id));
                        leptos::logging::warn!(
                            "canvas html frame: asset {asset_id} is {}, not text/html — \
                             leaving the placeholder",
                            asset.mime_type
                        );
                    }
                    Err(e) => {
                        let _ = cache.try_update_value(|c| c.abandon(&asset_id));
                        let _ = canvas.load_error.try_update(|v| {
                            *v = Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                                format!("Failed to load HTML frame content: {e}")
                            }));
                        });
                    }
                }
            });
        }
    });

    view! {
        <For
            each=move || {
                html_assets
                    .get()
                    .map(|(_, entries)| entries)
                    .unwrap_or_default()
            }
            key=|(shape_id, asset_id)| format!("{shape_id}:{asset_id}")
            children=move |(shape_id, asset_id): (String, String)| {
                let sid_for_bbox = shape_id.clone();
                let bbox = Memo::new(move |_| {
                    canvas.doc.with(|d| {
                        d.as_ref().and_then(|d| {
                            d.shapes
                                .iter()
                                .find(|s| s.id() == sid_for_bbox)
                                .map(Bbox::of_shape)
                        })
                    })
                });
                let selected = Memo::new(move |_| {
                    canvas.selection.with(|sel| sel.contains(&shape_id))
                });
                let srcdoc = Memo::new(move |_| {
                    cache_epoch.get();
                    cache
                        .try_with_value(|c| c.get(&asset_id).map(str::to_string))
                        .flatten()
                });
                view! {
                    <div
                        class="absolute"
                        style=move || {
                            bbox.get()
                                .map(|b| {
                                    format!(
                                        "left: {}px; top: {}px; width: {}px; height: {}px;",
                                        b.x, b.y, b.w, b.h,
                                    )
                                })
                                .unwrap_or_else(|| "display: none;".to_string())
                        }
                    >
                        {move || {
                            srcdoc
                                .get()
                                .map(|src| {
                                    view! {
                                        <iframe
                                            class="w-full h-full border-0 rounded-md bg-surface-raised"
                                            sandbox="allow-scripts"
                                            srcdoc=src
                                            style=move || {
                                                if selected.get() {
                                                    "pointer-events: auto;"
                                                } else {
                                                    "pointer-events: none;"
                                                }
                                            }
                                        />
                                    }
                                })
                        }}
                    </div>
                }
            }
        />
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{parse_path_d, Ease, FracIndex, Reveal, RevealMode};

    fn common(id: &str, reveal: Option<Reveal>) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            z: FracIndex::first(),
            parent_id: None,
            reveal,
        }
    }

    #[test]
    fn prompt_excerpt_truncates_on_char_boundaries() {
        assert_eq!(prompt_excerpt("short", 80), "short");
        let cjk = "画".repeat(100);
        let cut = prompt_excerpt(&cjk, 80);
        assert_eq!(cut.chars().count(), 81, "80 chars + ellipsis");
        assert!(cut.ends_with('…'));
    }

    /// Dashed and dotted are dash arrays; solid AND sketch draw a
    /// continuous stroke — sketch's look lives in the path data.
    #[test]
    fn stroke_kinds_map_to_dash_arrays_and_sketch_lives_in_the_path_data() {
        assert_eq!(stroke_dasharray(StrokeKind::Solid), None);
        assert_eq!(stroke_dasharray(StrokeKind::Sketch), None);
        assert_eq!(stroke_dasharray(StrokeKind::Dashed), Some("8 6"));
        assert_eq!(stroke_dasharray(StrokeKind::Dotted), Some("2 5"));
        let cmds = geo_path::geo_cmds(GeoForm::Rect, 40.0, 20.0);
        let clean = cmds_to_d(&cmds);
        for kind in [StrokeKind::Solid, StrokeKind::Dashed, StrokeKind::Dotted] {
            assert_eq!(outline_d(&cmds, kind, "s-1"), clean, "{kind:?}");
        }
        let sketched = outline_d(&cmds, StrokeKind::Sketch, "s-1");
        assert_ne!(sketched, clean);
        assert_eq!(sketched, sketch::sketch_d(&cmds, "s-1", OUTLINE_STROKE_W));
        assert!(parse_path_d(&sketched).is_ok());
    }

    /// Reveal hooks exist only for a shape that reveals, and `pathLength`
    /// only for the draw-on mode — the byte-identity of un-animated markup
    /// rests on these three `None`s.
    #[test]
    fn reveal_hooks_are_absent_without_a_reveal_and_path_length_is_draw_only() {
        assert_eq!(reveal_hooks(&common("a", None)), (None, None, None));
        let draw = Reveal {
            start_ms: 0,
            duration_ms: 10,
            ease: Ease::Linear,
            mode: RevealMode::Draw,
        };
        assert_eq!(
            reveal_hooks(&common("a", Some(draw))),
            (
                Some(reveal::OUTLINE_CLASS),
                Some("1"),
                Some(reveal::BODY_CLASS)
            )
        );
        let fade = Reveal {
            mode: RevealMode::Fade,
            ..draw
        };
        assert_eq!(
            reveal_hooks(&common("a", Some(fade))),
            (Some(reveal::OUTLINE_CLASS), None, Some(reveal::BODY_CLASS))
        );
    }

    #[test]
    fn palette_slots_resolve_to_theme_tokens_and_unknown_degrades() {
        assert_eq!(palette_var("red"), "var(--color-danger)");
        assert_eq!(palette_var("violet"), "var(--color-primary)");
        assert_eq!(palette_var(""), "var(--color-text-secondary)");
        assert_eq!(palette_var("hologram"), "var(--color-text-secondary)");
    }

    /// A `#rrggbb` literal (the contract's other admissible spelling) passes
    /// through both resolvers verbatim; anything hex-shaped but not six
    /// digits is an unknown slot and degrades like one.
    #[test]
    fn hex_colors_pass_through_and_near_misses_degrade() {
        assert!(is_hex_color("#A1b2C3"));
        assert!(!is_hex_color("#abc") && !is_hex_color("a1b2c3") && !is_hex_color("#gggggg"));
        assert_eq!(palette_var("#A1b2C3"), "#A1b2C3");
        assert_eq!(palette_var("#abc"), "var(--color-text-secondary)");
        let hex = ShapeStyle {
            color: "#123456".to_string(),
            ..ShapeStyle::default()
        };
        assert_eq!(text_fill(&hex), "#123456");
        assert_eq!(
            fill_css(&ShapeStyle { fill: true, ..hex }),
            "color-mix(in oklch, #123456 18%, transparent)"
        );
    }

    #[test]
    fn fill_css_is_none_unless_the_style_asks_for_a_fill() {
        let unfilled = ShapeStyle::default();
        assert_eq!(fill_css(&unfilled), "none");
        let filled = ShapeStyle {
            fill: true,
            color: "blue".to_string(),
            ..ShapeStyle::default()
        };
        assert_eq!(
            fill_css(&filled),
            "color-mix(in oklch, var(--color-info) 18%, transparent)"
        );
    }

    #[test]
    fn size_kinds_map_to_monotonic_font_and_stroke_scales() {
        assert!(font_size_for(SizeKind::Small) < font_size_for(SizeKind::Medium));
        assert!(font_size_for(SizeKind::Medium) < font_size_for(SizeKind::Large));
        assert!(ink_stroke_width(SizeKind::Small) < ink_stroke_width(SizeKind::Large));
    }

    /// The byte route is `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`
    /// and `asset_base` is minted as `/canvas-asset/<cap>/<canvas_id>`
    /// (server handler test `get_mints_an_asset_base_bound_to_the_canvas`) —
    /// the href is base + ONE path segment. A second `canvas_id` segment
    /// here would 404 every image.
    #[test]
    fn asset_href_is_the_minted_base_plus_one_path_segment() {
        assert_eq!(
            asset_href("/canvas-asset/cap123/cv-9", "aaaa.png"),
            "/canvas-asset/cap123/cv-9/aaaa.png"
        );
    }

    /// This file's production code (every `#[cfg(test)]`-gated item and every
    /// whole-line comment removed — this very test names the forbidden token,
    /// and the scanner judges code, not prose).
    ///
    /// Delegates to `i18n_census::production_lines`, this crate's one answer
    /// to "where does production code end". It walks gated ITEMS rather than
    /// cutting at the first `#[cfg(test)]` marker; the cut this replaced went
    /// blind the moment any gated item preceded the trailing test module, and
    /// went blind SILENTLY — a prefix cut can only ever under-scan, so the
    /// missed `<iframe` reads as "no second iframe" rather than as an error.
    /// `\r` stripping and the comment filter both live there now, so this
    /// function is not a second author of either.
    fn production_code() -> String {
        crate::i18n_census::production_lines(include_str!("shape_view.rs"))
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Source-level census: the one iframe is sandboxed with `allow-scripts`
    /// and NEVER `allow-same-origin`.
    ///
    /// # Why source-level
    ///
    /// The sandbox attribute is a string the compiler cannot check; adding
    /// `allow-same-origin` "to make the frame work" would compile, render,
    /// and hand model-authored HTML a same-origin document with reach into
    /// the Panel's storage and RPCs. At runtime that page looks identical
    /// until it is exploited.
    #[test]
    fn the_iframe_is_sandboxed_with_scripts_only_and_never_same_origin() {
        let code = production_code();
        assert_eq!(
            code.matches("<iframe").count(),
            1,
            "exactly one iframe production site in this file"
        );
        let at = code.find("<iframe").expect("counted above");
        let close = code[at..]
            .find("/>")
            .expect("the iframe element self-closes");
        let element = &code[at..at + close];
        assert!(
            element.contains("sandbox=\"allow-scripts\""),
            "the iframe must carry sandbox=\"allow-scripts\":\n{element}"
        );
        assert!(
            !element.contains("allow-same-origin"),
            "allow-same-origin would give model HTML the Panel's origin:\n{element}"
        );
        assert!(
            !code.contains("allow-same-origin"),
            "the token must not appear anywhere in this file's production code"
        );
    }

    /// …and no second iframe grows anywhere else in the Panel: every embed
    /// of model-authored HTML must go through the censused one above.
    #[test]
    fn no_iframe_exists_outside_the_censused_one() {
        let root = crate::disposed_reads::src_dir();
        let sources = crate::disposed_reads::rust_sources(&root);
        assert!(
            sources.len() > 50,
            "found almost no sources — the walk is broken, not the code"
        );
        let mut offenders = Vec::new();
        for path in sources {
            if path.ends_with("canvas/shape_view.rs") {
                continue; // the censused site
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            let code: String = src
                .replace('\r', "")
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            if code.contains("<iframe") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "iframes outside shape_view.rs — route model HTML through the \
             censused sandboxed frame:\n{}",
            offenders.join("\n")
        );
    }
}

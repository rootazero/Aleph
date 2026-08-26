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
//! `ShapeStyle.color` is a named palette slot on the wire; [`palette_var`]
//! resolves it to a `var(--color-*)` reference. Resolution must land in the
//! `style` attribute, not in `fill=`/`stroke=` presentation attributes —
//! CSS custom properties do not resolve inside bare SVG attributes, and a
//! literal `var(…)` there paints black in every browser.
//!
//! # Ink and arrows (Task 15)
//!
//! - `Ink` renders the pressure-aware freehand outline (`freehand.rs`) as a
//!   single filled path — the polygon is the stroke's silhouette.
//! - `Arrow` endpoints follow their bound shapes live: [`resolve_arrow_ends`]
//!   reads the bound shapes out of the document and [`arrow_anchor`] projects
//!   each endpoint onto the bound bbox's edge (center-to-edge intersection,
//!   aimed at the other end). The resolution sits behind a `Memo` so an
//!   unbound arrow never subscribes to the doc signal at all.
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
    AiFrameStatus, ArrowEnd, GeoForm, Shape, ShapeCommon, ShapeStyle, SizeKind,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::asset_ingest::SrcdocCache;
use super::freehand;
use super::interaction::Bbox;
use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, I18nCtx};
use crate::state::canvas::CanvasState;

/// Arrowhead length along the shaft, world units.
const ARROW_HEAD_LEN: f64 = 12.0;
/// Arrowhead half-width across the shaft, world units.
const ARROW_HEAD_HALF: f64 = 5.0;

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

/// Resolve a wire palette slot to a theme token reference.
///
/// Unknown slots (including the empty default) resolve to the neutral ink —
/// an unrecognized color must degrade to *visible*, never to an error.
#[must_use]
fn palette_var(slot: &str) -> &'static str {
    match slot {
        "red" => "var(--color-danger)",
        "orange" => "var(--color-warning)",
        "yellow" => "var(--color-chart-3)",
        "green" => "var(--color-success)",
        "blue" => "var(--color-info)",
        "violet" => "var(--color-primary)",
        _ => "var(--color-text-secondary)",
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
pub(super) fn text_fill(style: &ShapeStyle) -> &'static str {
    match style.color.as_str() {
        "red" | "orange" | "yellow" | "green" | "blue" | "violet" => palette_var(&style.color),
        _ => "var(--color-text-primary)",
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

/// Where an arrow endpoint bound to a shape attaches: the point where the
/// ray from the bbox's center toward `toward` crosses the bbox boundary.
/// Degenerate cases (a zero-extent box, `toward` at the center) answer the
/// center itself — an anchor must always exist.
#[must_use]
fn arrow_anchor(b: Bbox, toward: (f64, f64)) -> (f64, f64) {
    let (cx, cy) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    let (dx, dy) = (toward.0 - cx, toward.1 - cy);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return (cx, cy);
    }
    let tx = if dx.abs() > 1e-9 {
        (b.w / 2.0) / dx.abs()
    } else {
        f64::INFINITY
    };
    let ty = if dy.abs() > 1e-9 {
        (b.h / 2.0) / dy.abs()
    } else {
        f64::INFINITY
    };
    let t = tx.min(ty);
    if !t.is_finite() {
        return (cx, cy);
    }
    (cx + dx * t, cy + dy * t)
}

/// Resolve an arrow's endpoints against its bound shapes: a bound end
/// projects onto its shape's edge ([`arrow_anchor`]), aimed at the other
/// end's reference point (that end's bound shape's *center*, or its stored
/// coordinates). An end whose bound shape vanished falls back to its stored
/// x/y — the wire contract calls them "the recomputed fallback".
/// `pub(super)`: the export serializer resolves endpoints identically —
/// a second geometry would let the export and the live view disagree.
#[must_use]
pub(super) fn resolve_arrow_ends(
    shapes: &[Shape],
    start: &ArrowEnd,
    end: &ArrowEnd,
) -> ((f64, f64), (f64, f64)) {
    let bbox_of = |bind: &Option<String>| -> Option<Bbox> {
        bind.as_ref()
            .and_then(|id| shapes.iter().find(|s| s.id() == id))
            .map(Bbox::of_shape)
    };
    let center = |b: Bbox| (b.x + b.w / 2.0, b.y + b.h / 2.0);
    let start_bbox = bbox_of(&start.bind);
    let end_bbox = bbox_of(&end.bind);
    let start_ref = start_bbox.map_or((start.x, start.y), center);
    let end_ref = end_bbox.map_or((end.x, end.y), center);
    (
        start_bbox.map_or((start.x, start.y), |b| arrow_anchor(b, end_ref)),
        end_bbox.map_or((end.x, end.y), |b| arrow_anchor(b, start_ref)),
    )
}

/// `points=` polygon string for an arrowhead at `end`, pointing away from
/// `start`. Empty when the arrow is degenerate (zero length) — a polygon
/// with NaN vertices is an SVG parse error, not an invisible triangle.
/// `pub(super)`: shared with the export serializer (same head, same math).
#[must_use]
pub(super) fn arrow_head_points(start: (f64, f64), end: (f64, f64)) -> String {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return String::new();
    }
    let (ux, uy) = (dx / len, dy / len);
    let (bx, by) = (end.0 - ux * ARROW_HEAD_LEN, end.1 - uy * ARROW_HEAD_LEN);
    let (px, py) = (-uy, ux);
    format!(
        "{},{} {},{} {},{}",
        end.0,
        end.1,
        bx + px * ARROW_HEAD_HALF,
        by + py * ARROW_HEAD_HALF,
        bx - px * ARROW_HEAD_HALF,
        by - py * ARROW_HEAD_HALF
    )
}

/// `\n`-split text as a stack of `<text>` lines, first baseline at `y`.
fn text_block(x: f64, y: f64, text: &str, fs: f64, fill: &'static str) -> AnyView {
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

fn shape_svg(shape: &Shape, canvas: CanvasState, i18n: I18nCtx) -> AnyView {
    match shape {
        Shape::Geo {
            common,
            form,
            style,
            text,
        } => geo_svg(common, *form, style, text),
        Shape::Ink {
            common,
            style,
            points,
        } => ink_svg(common, style, points),
        Shape::Text {
            common,
            style,
            text,
        } => {
            let fs = font_size_for(style.size);
            view! {
                <g>{text_block(common.x, common.y + fs, text, fs, text_fill(style))}</g>
            }
            .into_any()
        }
        Shape::Note {
            common,
            style,
            text,
        } => note_svg(common, style, text),
        Shape::Image {
            common, asset_id, ..
        } => image_svg(common, asset_id, canvas),
        Shape::Frame { common, title, .. } => frame_svg(common, title),
        Shape::Html { common, .. } => html_placeholder_svg(common, i18n),
        Shape::Arrow {
            common: _,
            start,
            end,
            style,
            label,
        } => arrow_svg(start, end, style, label, canvas),
        Shape::AiImageFrame {
            common,
            prompt,
            status,
            ..
        } => ai_frame_svg(common, prompt, *status, i18n),
    }
}

fn geo_svg(common: &ShapeCommon, form: GeoForm, style: &ShapeStyle, text: &str) -> AnyView {
    let stroke = palette_var(&style.color);
    let paint = format!("stroke: {stroke}; fill: {};", fill_css(style));
    let fs = font_size_for(style.size);
    let outline = match form {
        GeoForm::Rect => view! {
            <rect
                x=common.x
                y=common.y
                width=common.w
                height=common.h
                rx=4
                style=paint
                stroke-width=2
            />
        }
        .into_any(),
        GeoForm::Ellipse => view! {
            <ellipse
                cx=common.x + common.w / 2.0
                cy=common.y + common.h / 2.0
                rx=common.w / 2.0
                ry=common.h / 2.0
                style=paint
                stroke-width=2
            />
        }
        .into_any(),
    };
    let label = (!text.is_empty()).then(|| {
        text_block(
            common.x + 10.0,
            common.y + fs + 8.0,
            text,
            fs,
            text_fill(style),
        )
    });
    view! { <g>{outline}{label}</g> }.into_any()
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

fn note_svg(common: &ShapeCommon, style: &ShapeStyle, text: &str) -> AnyView {
    // A note is always a filled card. The default slot reads as the classic
    // sticky yellow; named slots wash the card in their own color.
    let base = match style.color.as_str() {
        "" | "default" => "var(--color-warning)",
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

fn arrow_svg(
    start: &ArrowEnd,
    end: &ArrowEnd,
    style: &ShapeStyle,
    label: &str,
    canvas: CanvasState,
) -> AnyView {
    let stroke = palette_var(&style.color);
    // Bound endpoints re-resolve whenever the document changes (the bound
    // shape may have moved), so the resolution reads the doc signal — but
    // only when a binding exists: an unbound arrow must not re-render on
    // every unrelated edit. The Memo dedupes by value, so doc churn that
    // leaves the anchors unchanged updates nothing downstream.
    let has_binds = start.bind.is_some() || end.bind.is_some();
    let (start, end) = (start.clone(), end.clone());
    let raw = ((start.x, start.y), (end.x, end.y));
    let ends: Memo<((f64, f64), (f64, f64))> = Memo::new(move |_| {
        if !has_binds {
            return raw;
        }
        canvas.doc.with(|d| {
            let shapes = d.as_ref().map_or(&[][..], |d| d.shapes.as_slice());
            resolve_arrow_ends(shapes, &start, &end)
        })
    });
    let label = label.to_string();
    view! {
        <g>
            <line
                x1=move || ends.get().0.0
                y1=move || ends.get().0.1
                x2=move || ends.get().1.0
                y2=move || ends.get().1.1
                style=format!("stroke: {stroke};")
                stroke-width=2
            />
            {move || {
                let (s, e) = ends.get();
                let head = arrow_head_points(s, e);
                (!head.is_empty())
                    .then(|| view! { <polygon points=head style=format!("fill: {stroke};") /> })
            }}
            {(!label.is_empty()).then(|| {
                view! {
                    <text
                        x=move || (ends.get().0.0 + ends.get().1.0) / 2.0
                        y=move || (ends.get().0.1 + ends.get().1.1) / 2.0 - 6.0
                        text-anchor="middle"
                        font-size=12
                        style=format!("fill: {stroke}; user-select: none;")
                    >
                        {label}
                    </text>
                }
            })}
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

    fn bbox(x: f64, y: f64, w: f64, h: f64) -> Bbox {
        Bbox { x, y, w, h }
    }

    #[track_caller]
    fn assert_close(got: (f64, f64), want: (f64, f64)) {
        assert!(
            (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
            "got {got:?}, want {want:?}"
        );
    }

    #[test]
    fn arrow_anchor_lands_on_the_correct_edge_in_all_four_quadrants() {
        // Box (0,0)–(100,60), center (50,30). Each target sits in a
        // different quadrant relative to the center; the anchor must land on
        // the boundary, on that quadrant's side.
        let b = bbox(0.0, 0.0, 100.0, 60.0);
        // NE, steep: exits the top edge, right half.
        assert_close(arrow_anchor(b, (200.0, -120.0)), (80.0, 0.0));
        // SE, shallow: exits the right edge, lower half.
        assert_close(arrow_anchor(b, (250.0, 130.0)), (100.0, 55.0));
        // SW, steep: exits the bottom edge, left half.
        assert_close(arrow_anchor(b, (-150.0, 230.0)), (20.0, 60.0));
        // NW, diagonal: exits the top edge, left half.
        assert_close(arrow_anchor(b, (-50.0, -70.0)), (20.0, 0.0));
    }

    #[test]
    fn arrow_anchor_degenerate_targets_answer_the_center() {
        let b = bbox(0.0, 0.0, 100.0, 60.0);
        assert_eq!(
            arrow_anchor(b, (50.0, 30.0)),
            (50.0, 30.0),
            "toward = center"
        );
        let hairline = bbox(10.0, 10.0, 0.0, 0.0);
        assert_eq!(arrow_anchor(hairline, (99.0, 99.0)), (10.0, 10.0));
    }

    #[test]
    fn resolve_arrow_ends_follows_bound_shapes_and_falls_back_when_they_vanish() {
        let target = Shape::Note {
            common: ShapeCommon {
                id: "n1".to_string(),
                x: 200.0,
                y: 0.0,
                w: 100.0,
                h: 60.0,
                z: aleph_protocol::canvas::FracIndex::first(),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: String::new(),
        };
        let start = ArrowEnd {
            x: 0.0,
            y: 30.0,
            bind: None,
        };
        let end = ArrowEnd {
            x: 210.0, // stale drawn coordinate — the binding overrides it
            y: 10.0,
            bind: Some("n1".to_string()),
        };
        let (s, e) = resolve_arrow_ends(std::slice::from_ref(&target), &start, &end);
        assert_eq!(s, (0.0, 30.0), "an unbound end keeps its coordinates");
        // The bound end sits on the shape's near edge, aimed at the start.
        assert_close(e, (200.0, 30.0));
        // The bound shape vanished (deleted by a broadcast): stored x/y is
        // the documented fallback.
        let (_, e) = resolve_arrow_ends(&[], &start, &end);
        assert_eq!(e, (210.0, 10.0));
    }

    #[test]
    fn prompt_excerpt_truncates_on_char_boundaries() {
        assert_eq!(prompt_excerpt("short", 80), "short");
        let cjk = "画".repeat(100);
        let cut = prompt_excerpt(&cjk, 80);
        assert_eq!(cut.chars().count(), 81, "80 chars + ellipsis");
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn arrow_head_is_symmetric_and_empty_for_a_degenerate_arrow() {
        assert_eq!(
            arrow_head_points((3.0, 4.0), (3.0, 4.0)),
            "",
            "a zero-length arrow must not emit NaN vertices"
        );
        // Horizontal arrow → barbs mirror across the shaft.
        let pts = arrow_head_points((0.0, 0.0), (100.0, 0.0));
        assert_eq!(pts, "100,0 88,5 88,-5");
    }

    #[test]
    fn palette_slots_resolve_to_theme_tokens_and_unknown_degrades() {
        assert_eq!(palette_var("red"), "var(--color-danger)");
        assert_eq!(palette_var("violet"), "var(--color-primary)");
        assert_eq!(palette_var(""), "var(--color-text-secondary)");
        assert_eq!(palette_var("hologram"), "var(--color-text-secondary)");
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

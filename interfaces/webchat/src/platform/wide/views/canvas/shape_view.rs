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
//! # What is deliberately simple in this task
//!
//! - `Ink` renders the raw polyline; the pressure-aware freehand outline is
//!   Task 15.
//! - `Html` is a labelled placeholder box; the sandboxed iframe (with its
//!   `allow-scripts`-only census) is Task 16.
//! - Text does not wrap — explicit `\n` breaks only, the `plan_dag.rs`
//!   limitation. The text-editing overlay (Task 14) owns real layout.

use aleph_protocol::canvas::{
    AiFrameStatus, ArrowEnd, GeoForm, Shape, ShapeCommon, ShapeStyle, SizeKind,
};
use leptos::prelude::*;

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
#[must_use]
fn text_fill(style: &ShapeStyle) -> &'static str {
    match style.color.as_str() {
        "red" | "orange" | "yellow" | "green" | "blue" | "violet" => palette_var(&style.color),
        _ => "var(--color-text-primary)",
    }
}

#[must_use]
fn font_size_for(size: SizeKind) -> f64 {
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

/// Path data for an ink polyline (points are shape-origin-relative).
///
/// Empty points yield an empty string (the caller skips the `<path>`); a
/// single point becomes a zero-length segment, which `stroke-linecap: round`
/// renders as a dot — a tap with a pen must leave a mark, not a panic.
#[must_use]
fn ink_path_d(points: &[[f32; 3]]) -> String {
    let mut iter = points.iter();
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut d = format!("M {} {}", first[0], first[1]);
    let mut segments = 0usize;
    for p in iter {
        d.push_str(&format!(" L {} {}", p[0], p[1]));
        segments += 1;
    }
    if segments == 0 {
        // Zero-length segment: linecap "round" renders it as a dot.
        d.push_str(&format!(" L {} {}", first[0], first[1]));
    }
    d
}

/// First `max` chars of an AI prompt, `…`-terminated — char-boundary safe
/// (a CJK prompt sliced by bytes would panic).
#[must_use]
fn prompt_excerpt(prompt: &str, max: usize) -> String {
    if prompt.chars().count() <= max {
        return prompt.to_string();
    }
    let mut s: String = prompt.chars().take(max).collect();
    s.push('…');
    s
}

/// `points=` polygon string for an arrowhead at `end`, pointing away from
/// `start`. Empty when the arrow is degenerate (zero length) — a polygon
/// with NaN vertices is an SVG parse error, not an invisible triangle.
#[must_use]
fn arrow_head_points(start: (f64, f64), end: (f64, f64)) -> String {
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
        } => arrow_svg(start, end, style, label),
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
    let d = ink_path_d(points);
    if d.is_empty() {
        return ().into_any();
    }
    view! {
        <g transform=format!("translate({} {})", common.x, common.y)>
            <path
                d=d
                fill="none"
                style=format!("stroke: {};", palette_var(&style.color))
                stroke-width=ink_stroke_width(style.size)
                stroke-linecap="round"
                stroke-linejoin="round"
            />
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
            let href = format!("{base}/{asset_id}");
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

fn arrow_svg(start: &ArrowEnd, end: &ArrowEnd, style: &ShapeStyle, label: &str) -> AnyView {
    let stroke = palette_var(&style.color);
    let head = arrow_head_points((start.x, start.y), (end.x, end.y));
    view! {
        <g>
            <line
                x1=start.x
                y1=start.y
                x2=end.x
                y2=end.y
                style=format!("stroke: {stroke};")
                stroke-width=2
            />
            {(!head.is_empty()).then(|| view! {
                <polygon points=head style=format!("fill: {stroke};") />
            })}
            {(!label.is_empty()).then(|| {
                let label = label.to_string();
                view! {
                    <text
                        x=(start.x + end.x) / 2.0
                        y=(start.y + end.y) / 2.0 - 6.0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_path_d_handles_empty_and_single_point_without_panicking() {
        assert_eq!(ink_path_d(&[]), "", "no points, no path");
        assert_eq!(
            ink_path_d(&[[4.0, 5.0, 0.5]]),
            "M 4 5 L 4 5",
            "a single point must become a zero-length dot segment"
        );
    }

    #[test]
    fn ink_path_d_chains_line_segments_in_order() {
        assert_eq!(
            ink_path_d(&[[0.0, 0.0, 0.5], [10.0, 0.0, 0.5], [10.0, 20.0, 0.5]]),
            "M 0 0 L 10 0 L 10 20"
        );
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
}

//! Rasterizing export: selected shapes → standalone SVG string → PNG.
//!
//! # The pipeline
//!
//! 1. [`export_svg`] — a **pure string serializer** mirroring the live SVG
//!    renderer (`shape_view.rs`) arm for arm, with two deliberate
//!    differences: image hrefs are inlined as `data:` URLs (a capability URL
//!    in the export would 401 for anyone else — and an external href would
//!    taint the rasterizing canvas, making `to_data_url` throw), and colors
//!    are concrete hex values (below).
//! 2. [`rasterize_svg_to_png`] — SVG string → `data:` URL (CSP: no `blob:`) →
//!    `HtmlImageElement::decode()` → `CanvasRenderingContext2d::draw_image`
//!    → `to_data_url("image/png")`.
//! 3. [`download_png`] — the `transcript.rs` blob/anchor idiom, via the
//!    voice module's byte helpers.
//!
//! The annotation flow (`ai.rs`) reuses steps 1–2 to composite an image with
//! its annotation marks.
//!
//! # Why this file spells hex colors (and the theme rule does not apply)
//!
//! The Panel's hardcoded-color rule exists so themed UI follows the
//! `--color-*` tokens. An export is not themed UI: the SVG is rasterized as
//! a **standalone document** in an off-DOM image, where the app's custom
//! properties do not exist — `var(--color-primary)` resolves to nothing and
//! paints black. The palette below is the light-theme reading of the same
//! slots the live renderer maps through `shape_view::palette_var`, fixed at
//! export time the way a printed page fixes its ink.

use std::collections::HashMap;
use std::fmt::Write as _;

use aleph_protocol::canvas::{GeoForm, Shape, ShapeStyle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use super::interaction::{self, Bbox};
use super::{asset_ingest, freehand, shape_view};
use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::state::canvas::CanvasState;

/// World-unit padding around an exported selection.
pub(super) const EXPORT_PAD: f64 = 16.0;
/// Longest raster edge, px — past this the bitmap scales down (canvas
/// allocations near the browser's texture limits fail silently).
pub(super) const RASTER_MAX_EDGE: f64 = 4096.0;

// Export palette — hex on purpose (module doc). Light-theme readings of the
// slots `shape_view::palette_var` maps to theme tokens.
const EXPORT_BG: &str = "#ffffff";
const EXPORT_TEXT: &str = "#1f2328";
const EXPORT_MUTED: &str = "#57606a";
const EXPORT_BORDER: &str = "#8c959f";
const EXPORT_SURFACE: &str = "#f6f8fa";
const EXPORT_AI_ACCENT: &str = "#6e56cf";

fn export_stroke(slot: &str) -> &'static str {
    match slot {
        "red" => "#e5484d",
        "orange" => "#f5a623",
        "yellow" => "#d4a72c",
        "green" => "#30a46c",
        "blue" => "#0091ff",
        "violet" => "#6e56cf",
        _ => "#6e7781",
    }
}

fn export_text_fill(style: &ShapeStyle) -> &'static str {
    match style.color.as_str() {
        "red" | "orange" | "yellow" | "green" | "blue" | "violet" => export_stroke(&style.color),
        _ => EXPORT_TEXT,
    }
}

/// Fill for a closed shape: an alpha wash of its stroke (#RRGGBBAA ≈ the
/// live renderer's 18% color-mix), or none.
fn export_fill(style: &ShapeStyle) -> String {
    if style.fill {
        format!("{}2e", export_stroke(&style.color))
    } else {
        "none".to_string()
    }
}

/// Minimal XML text/attribute escaping — model- and user-authored text goes
/// into the markup, and a stray `<` must become character data, not a tag.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Raster size for a world-unit box: 1 world unit = 1 px, capped so the long
/// edge stays within [`RASTER_MAX_EDGE`], floored at 1×1.
#[must_use]
pub(super) fn raster_dimensions(w: f64, h: f64) -> (u32, u32) {
    let long = w.max(h).max(1.0);
    let scale = if long > RASTER_MAX_EDGE {
        RASTER_MAX_EDGE / long
    } else {
        1.0
    };
    let px = |v: f64| (v * scale).round().max(1.0) as u32;
    (px(w.max(1.0)), px(h.max(1.0)))
}

/// `\n`-split text as stacked `<text>` lines — the string twin of
/// `shape_view::text_block`.
fn push_text_block(out: &mut String, x: f64, first_baseline: f64, text: &str, fs: f64, fill: &str) {
    let line_height = fs * 1.4;
    for (i, line) in text.split('\n').enumerate() {
        let _ = write!(
            out,
            "<text x=\"{x}\" y=\"{}\" font-size=\"{fs}\" fill=\"{fill}\">{}</text>",
            first_baseline + line_height * i as f64,
            xml_escape(line)
        );
    }
}

/// Serialize `layers` (already in draw order) into a standalone `<svg>`
/// document over `viewbox` (+`pad`).
///
/// `all_shapes` is the whole document — bound arrow endpoints resolve
/// against shapes that may not themselves be exported. `assets` maps asset
/// id → `data:` URL; an image whose asset is missing renders the same
/// dashed placeholder the live view uses, never a capability URL.
#[must_use]
pub(super) fn export_svg(
    layers: &[Shape],
    all_shapes: &[Shape],
    assets: &HashMap<String, String>,
    viewbox: Bbox,
    pad: f64,
) -> String {
    let x = viewbox.x - pad;
    let y = viewbox.y - pad;
    let w = (viewbox.w + pad * 2.0).max(1.0);
    let h = (viewbox.h + pad * 2.0).max(1.0);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"{x} {y} {w} {h}\" font-family=\"ui-sans-serif, system-ui, sans-serif\">"
    );
    // Opaque ground: a transparent export pasted into a dark surface reads
    // as broken, and the annotation composite must show the image on paper.
    let _ = write!(
        out,
        "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"{EXPORT_BG}\"/>"
    );
    for shape in layers {
        push_shape(&mut out, shape, all_shapes, assets);
    }
    out.push_str("</svg>");
    out
}

/// One shape's markup — the string twin of `shape_view::shape_svg`, arm for
/// arm.
fn push_shape(
    out: &mut String,
    shape: &Shape,
    all_shapes: &[Shape],
    assets: &HashMap<String, String>,
) {
    match shape {
        Shape::Geo {
            common,
            form,
            style,
            text,
        } => {
            let stroke = export_stroke(&style.color);
            let fill = export_fill(style);
            match form {
                GeoForm::Rect => {
                    let _ = write!(
                        out,
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"4\" \
                         fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"2\"/>",
                        common.x, common.y, common.w, common.h
                    );
                }
                GeoForm::Ellipse => {
                    let _ = write!(
                        out,
                        "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" \
                         fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"2\"/>",
                        common.x + common.w / 2.0,
                        common.y + common.h / 2.0,
                        common.w / 2.0,
                        common.h / 2.0
                    );
                }
            }
            if !text.is_empty() {
                let fs = shape_view::font_size_for(style.size);
                push_text_block(
                    out,
                    common.x + 10.0,
                    common.y + fs + 8.0,
                    text,
                    fs,
                    export_text_fill(style),
                );
            }
        }
        Shape::Ink {
            common,
            style,
            points,
        } => {
            let d = freehand::outline_path_d(points, shape_view::freehand_size(style.size));
            if !d.is_empty() {
                let _ = write!(
                    out,
                    "<g transform=\"translate({} {})\"><path d=\"{d}\" fill=\"{}\"/></g>",
                    common.x,
                    common.y,
                    export_stroke(&style.color)
                );
            }
        }
        Shape::Text {
            common,
            style,
            text,
        } => {
            let fs = shape_view::font_size_for(style.size);
            push_text_block(
                out,
                common.x,
                common.y + fs,
                text,
                fs,
                export_text_fill(style),
            );
        }
        Shape::Note {
            common,
            style,
            text,
        } => {
            let base = match style.color.as_str() {
                "" | "default" => "#f5a623",
                _ => export_stroke(&style.color),
            };
            let _ = write!(
                out,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"8\" \
                 fill=\"{base}38\" stroke=\"{base}73\" stroke-width=\"1\"/>",
                common.x, common.y, common.w, common.h
            );
            if !text.is_empty() {
                let fs = shape_view::font_size_for(style.size);
                push_text_block(
                    out,
                    common.x + 12.0,
                    common.y + fs + 10.0,
                    text,
                    fs,
                    EXPORT_TEXT,
                );
            }
        }
        Shape::Image {
            common, asset_id, ..
        } => match assets.get(asset_id) {
            Some(data_url) => {
                let _ = write!(
                    out,
                    "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{}\" \
                     preserveAspectRatio=\"xMidYMid meet\"/>",
                    common.x,
                    common.y,
                    common.w,
                    common.h,
                    xml_escape(data_url)
                );
            }
            None => {
                let _ = write!(
                    out,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"4\" \
                     fill=\"{EXPORT_SURFACE}\" stroke=\"{EXPORT_BORDER}\" stroke-width=\"1\" \
                     stroke-dasharray=\"6 4\"/>",
                    common.x, common.y, common.w, common.h
                );
            }
        },
        Shape::Frame { common, title, .. } => {
            let _ = write!(
                out,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                 fill=\"{EXPORT_BG}\" stroke=\"{EXPORT_BORDER}\" stroke-width=\"2\"/>",
                common.x, common.y, common.w, common.h
            );
            if !title.is_empty() {
                let _ = write!(
                    out,
                    "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{EXPORT_MUTED}\">{}</text>",
                    common.x,
                    common.y - 8.0,
                    xml_escape(title)
                );
            }
        }
        Shape::Html { common, .. } => {
            // Live HTML cannot ride into a static bitmap — the placeholder
            // box (the live renderer's fallback) stands in.
            let _ = write!(
                out,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"6\" \
                 fill=\"{EXPORT_SURFACE}\" stroke=\"{EXPORT_BORDER}\" stroke-width=\"1\"/>",
                common.x, common.y, common.w, common.h
            );
        }
        Shape::Arrow {
            start,
            end,
            style,
            label,
            ..
        } => {
            let (s, e) = shape_view::resolve_arrow_ends(all_shapes, start, end);
            let stroke = export_stroke(&style.color);
            let _ = write!(
                out,
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{stroke}\" \
                 stroke-width=\"2\"/>",
                s.0, s.1, e.0, e.1
            );
            let head = shape_view::arrow_head_points(s, e);
            if !head.is_empty() {
                let _ = write!(out, "<polygon points=\"{head}\" fill=\"{stroke}\"/>");
            }
            if !label.is_empty() {
                let _ = write!(
                    out,
                    "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"12\" \
                     fill=\"{stroke}\">{}</text>",
                    (s.0 + e.0) / 2.0,
                    (s.1 + e.1) / 2.0 - 6.0,
                    xml_escape(label)
                );
            }
        }
        Shape::AiImageFrame { common, prompt, .. } => {
            let _ = write!(
                out,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"6\" fill=\"none\" \
                 stroke=\"{EXPORT_AI_ACCENT}\" stroke-width=\"2\" stroke-dasharray=\"8 5\"/>",
                common.x, common.y, common.w, common.h
            );
            let _ = write!(
                out,
                "<text x=\"{}\" y=\"{}\" font-size=\"13\" fill=\"{EXPORT_MUTED}\">{}</text>",
                common.x + 12.0,
                common.y + 24.0,
                xml_escape(&shape_view::prompt_excerpt(prompt, 80))
            );
        }
    }
}

/// Rasterize a standalone SVG document to a PNG `data:` URL.
///
/// WASM-only glue: `data:` URL → `HtmlImageElement::decode()` (the same
/// off-DOM decode `editor.rs::natural_image_size` uses) → 2D canvas →
/// `to_data_url`. Every failure is a `String` for the caller to classify;
/// nothing here unwraps — a panic takes the whole Panel down.
///
/// The SVG travels as `data:image/svg+xml` (percent-encoded, Unicode-safe)
/// and NOT as a `blob:` object URL: the Panel ships under a CSP whose
/// `img-src 'self' data: https:` has no `blob:` source, so a blob-URL image
/// load is blocked by policy — real-machine QA caught exactly that. `data:`
/// is inside the policy; widening the CSP for an internal pipeline would be
/// the wrong direction.
pub(super) async fn rasterize_svg_to_png(
    svg: &str,
    out_w: u32,
    out_h: u32,
) -> Result<String, String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| "no document".to_string())?;
    let encoded: String = js_sys::encode_uri_component(svg).into();
    let url = format!("data:image/svg+xml;charset=utf-8,{encoded}");
    let img = web_sys::HtmlImageElement::new()
        .map_err(|_| "could not create an image element".to_string())?;
    img.set_src(&url);
    let decoded = wasm_bindgen_futures::JsFuture::from(img.decode()).await;
    decoded.map_err(|_| "the browser could not decode the export SVG".to_string())?;

    let canvas_el = document
        .create_element("canvas")
        .ok()
        .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| "could not create a canvas".to_string())?;
    canvas_el.set_width(out_w);
    canvas_el.set_height(out_h);
    let ctx = canvas_el
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
        .ok_or_else(|| "no 2d context".to_string())?;
    ctx.draw_image_with_html_image_element_and_dw_and_dh(
        &img,
        0.0,
        0.0,
        f64::from(out_w),
        f64::from(out_h),
    )
    .map_err(|_| "could not draw the SVG".to_string())?;
    canvas_el
        .to_data_url_with_type("image/png")
        .map_err(|_| "PNG encoding failed".to_string())
}

/// Hand a PNG `data:` URL to the browser as a download — the
/// `transcript.rs` blob/anchor idiom, through the voice module's byte
/// helpers (a large `data:` href is unreliable where a `blob:` URL is not).
pub(super) fn download_png(png_data_url: &str, filename: &str) {
    let Some(b64) = asset_ingest::data_url_base64(png_data_url) else {
        return;
    };
    let Some(bytes) = crate::views::voice::audio::base64_to_bytes(b64) else {
        return;
    };
    let Some(url) = crate::views::voice::audio::bytes_to_object_url(&bytes, "image/png") else {
        return;
    };
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let link = match document.create_element("a") {
        Ok(el) => match el.dyn_into::<web_sys::HtmlAnchorElement>() {
            Ok(anchor) => anchor,
            Err(_) => return,
        },
        Err(_) => return,
    };
    link.set_href(&url);
    link.set_download(filename);
    let _ = document.body().map(|body| body.append_child(&link));
    link.click();
    let _ = document.body().map(|body| body.remove_child(&link));
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// "Export PNG" — visible while the selection is non-empty. Fetches the
/// selected images' bytes, serializes, rasterizes, downloads.
#[component]
pub(super) fn ExportPngButton() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();
    let busy = RwSignal::new(false);

    let has_selection = Memo::new(move |_| !canvas.selection.with(Vec::is_empty));

    let on_click = move |_| {
        if busy.get_untracked() {
            return;
        }
        let Some(cid) = canvas.open_canvas.get_untracked() else {
            return;
        };
        let sel = canvas.selection.get_untracked();
        // Selected shapes in z order (the editor's sort — one ordering rule),
        // plus the whole document for arrow-bind resolution.
        let data = canvas.doc.with_untracked(|d| {
            d.as_ref().map(|d| {
                let ordered: Vec<Shape> = super::editor::z_sorted_ids(&d.shapes)
                    .into_iter()
                    .filter(|id| sel.iter().any(|s| s == id))
                    .filter_map(|id| d.shapes.iter().find(|s| s.id() == id).cloned())
                    .collect();
                (ordered, d.shapes.clone())
            })
        });
        let Some((layers, all_shapes)) = data else {
            return;
        };
        let Some(viewbox) = interaction::selection_bbox(&all_shapes, &sel) else {
            return;
        };
        if layers.is_empty() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let mut assets: HashMap<String, String> = HashMap::new();
            for shape in &layers {
                let Shape::Image { asset_id, .. } = shape else {
                    continue;
                };
                if assets.contains_key(asset_id) {
                    continue;
                }
                match CanvasApi::asset_get(&state, &cid, asset_id).await {
                    Ok(asset) => {
                        assets.insert(
                            asset_id.clone(),
                            format!("data:{};base64,{}", asset.mime_type, asset.data),
                        );
                    }
                    Err(e) => {
                        let _ = canvas.load_error.try_update(|v| {
                            *v = Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                                format!("Failed to load an image for export: {e}")
                            }));
                        });
                        let _ = busy.try_set(false);
                        return;
                    }
                }
            }
            let svg = export_svg(&layers, &all_shapes, &assets, viewbox, EXPORT_PAD);
            let (pw, ph) =
                raster_dimensions(viewbox.w + EXPORT_PAD * 2.0, viewbox.h + EXPORT_PAD * 2.0);
            match rasterize_svg_to_png(&svg, pw, ph).await {
                Ok(data_url) => {
                    let timestamp = js_sys::Date::new_0()
                        .to_iso_string()
                        .as_string()
                        .unwrap_or_default()
                        .replace(':', "-");
                    download_png(&data_url, &format!("aleph-canvas-{timestamp}.png"));
                }
                Err(e) => {
                    let _ = canvas.load_error.try_update(|v| {
                        *v = Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                            format!("Failed to export the selection: {e}")
                        }));
                    });
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        {move || {
            has_selection.get().then(|| view! {
                <button
                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border \
                           bg-surface-raised text-xs font-medium text-text-secondary \
                           hover:text-text-primary hover:border-primary/50 shadow-sm \
                           transition-colors disabled:opacity-50"
                    prop:disabled=move || busy.get()
                    on:click=on_click
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                        <polyline points="7 10 12 15 17 10" />
                        <line x1="12" y1="15" x2="12" y2="3" />
                    </svg>
                    {t!(i18n, canvas.export_png)}
                </button>
            })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon};

    fn common(id: &str, x: f64, y: f64, w: f64, h: f64) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x,
            y,
            w,
            h,
            z: FracIndex::first(),
            parent_id: None,
        }
    }

    fn image(id: &str, asset: &str) -> Shape {
        Shape::Image {
            common: common(id, 0.0, 0.0, 100.0, 80.0),
            asset_id: asset.to_string(),
            natural_w: 0.0,
            natural_h: 0.0,
        }
    }

    const VIEW: Bbox = Bbox {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 80.0,
    };

    /// The export embeds image bytes as `data:` URLs — never the capability
    /// URL, which would 401 for anyone else and taint the rasterizing
    /// canvas.
    #[test]
    fn svg_export_embeds_images_as_data_urls() {
        let shapes = vec![image("i1", "abc.png")];
        let mut assets = HashMap::new();
        assets.insert(
            "abc.png".to_string(),
            "data:image/png;base64,AAAA".to_string(),
        );
        let svg = export_svg(&shapes, &shapes, &assets, VIEW, 0.0);
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""),
            "{svg}"
        );
        assert!(
            svg.contains("href=\"data:image/png;base64,AAAA\""),
            "the image must ride inline: {svg}"
        );
        assert!(
            !svg.contains("canvas-asset"),
            "no capability URL may leak into an export: {svg}"
        );
    }

    /// An image whose bytes were not fetched renders a placeholder box, not
    /// a broken external reference.
    #[test]
    fn a_missing_asset_renders_a_placeholder_never_an_external_href() {
        let shapes = vec![image("i1", "abc.png")];
        let svg = export_svg(&shapes, &shapes, &HashMap::new(), VIEW, 0.0);
        assert!(!svg.contains("<image"), "{svg}");
        assert!(svg.contains("stroke-dasharray=\"6 4\""), "{svg}");
    }

    /// Model- and user-authored text is escaped into character data.
    #[test]
    fn text_content_is_xml_escaped() {
        let shapes = vec![Shape::Text {
            common: common("t1", 0.0, 0.0, 100.0, 40.0),
            style: ShapeStyle::default(),
            text: "<script>&\"attack\"</script>".to_string(),
        }];
        let svg = export_svg(&shapes, &shapes, &HashMap::new(), VIEW, 0.0);
        assert!(!svg.contains("<script>"), "{svg}");
        assert!(
            svg.contains("&lt;script&gt;&amp;&quot;attack&quot;&lt;/script&gt;"),
            "{svg}"
        );
    }

    /// The root carries an intrinsic size equal to the padded viewBox —
    /// browsers refuse to draw an SVG image with no intrinsic size.
    #[test]
    fn the_root_svg_carries_viewbox_and_matching_intrinsic_size() {
        let svg = export_svg(&[], &[], &HashMap::new(), VIEW, 16.0);
        assert!(svg.contains("viewBox=\"-16 -16 132 112\""), "{svg}");
        assert!(svg.contains("width=\"132\" height=\"112\""), "{svg}");
        assert!(svg.ends_with("</svg>"), "{svg}");
    }

    /// Bound arrow endpoints resolve against the whole document, so an
    /// exported arrow pointing at a non-exported shape still lands on that
    /// shape's edge instead of its stale stored coordinates.
    #[test]
    fn arrows_resolve_bindings_against_the_whole_document() {
        let target = Shape::Geo {
            common: common("g1", 200.0, 0.0, 40.0, 40.0),
            form: GeoForm::Rect,
            style: ShapeStyle::default(),
            text: String::new(),
        };
        let arrow = Shape::Arrow {
            common: common("a1", 0.0, 0.0, 1.0, 1.0),
            start: aleph_protocol::canvas::ArrowEnd {
                x: 0.0,
                y: 20.0,
                bind: None,
            },
            end: aleph_protocol::canvas::ArrowEnd {
                x: 999.0, // stale fallback — the binding must win
                y: 999.0,
                bind: Some("g1".to_string()),
            },
            style: ShapeStyle::default(),
            label: String::new(),
        };
        let all = vec![arrow.clone(), target];
        let svg = export_svg(&[arrow], &all, &HashMap::new(), VIEW, 0.0);
        assert!(
            svg.contains("x2=\"200\" y2=\"20\""),
            "the bound end must land on the target's left edge: {svg}"
        );
        assert!(!svg.contains("999"), "{svg}");
    }

    /// The raster cap scales the long edge down to the limit and floors
    /// degenerate boxes at one pixel.
    #[test]
    fn raster_dimensions_cap_the_long_edge_and_floor_at_one_pixel() {
        assert_eq!(raster_dimensions(100.0, 80.0), (100, 80));
        let (w, h) = raster_dimensions(8192.0, 4096.0);
        assert_eq!(w, 4096);
        assert_eq!(h, 2048);
        assert_eq!(raster_dimensions(0.0, 0.0), (1, 1));
    }

    /// Ink strokes export as the same filled freehand outline the live
    /// renderer draws — not a polyline.
    #[test]
    fn ink_exports_the_freehand_outline_as_a_filled_path() {
        let shapes = vec![Shape::Ink {
            common: common("k1", 0.0, 0.0, 60.0, 20.0),
            style: ShapeStyle::default(),
            points: vec![[0.0, 0.0, 0.5], [50.0, 10.0, 0.5]],
        }];
        let svg = export_svg(&shapes, &shapes, &HashMap::new(), VIEW, 0.0);
        assert!(svg.contains("<path d=\"M"), "{svg}");
        assert!(svg.contains("fill=\"#6e7781\""), "{svg}");
        assert!(!svg.contains("polyline"), "{svg}");
    }
}

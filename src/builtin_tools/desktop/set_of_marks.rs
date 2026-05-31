//! `desktop_som` — native visual **Set-of-Marks** for GUI grounding.
//!
//! Vision models drive a GUI far more reliably when each actionable element is
//! visibly tagged with a number on the screenshot itself (the "Set-of-Marks"
//! prompting technique). Aleph already produces the *data* half of this —
//! [`super::ax`]'s `desktop_ax_snapshot` returns an indexed element list with
//! pre-computed click `center`s — but the model had to eyeball which pixel a
//! given index referred to. This tool overlays those same indices onto the
//! capture, so the model can say "click 7" and read element 7's `center`
//! straight out of the response.
//!
//! Pipeline (all reused infrastructure except the pure overlay):
//!   1. `ax.query_tree` → frontmost (or `pid`) app's AX subtree.
//!   2. [`super::interactable::collect_interactable`] → actionable elements.
//!   3. `screen.screenshot` → PNG capture (carries the Retina `scale_factor`).
//!   4. [`annotate`] → numbered marks painted onto the capture (pure Rust,
//!      no font/drawing dependency — a hand-rolled 3×5 bitmap digit font).
//!   5. `perception::process_screenshot` → honour `format`/`max_width` and the
//!      shared result-size budget, exactly like the `screenshot` action.
//!
//! Element `center`s are returned in **screen points** (unscaled), so they
//! feed directly into the `desktop` tool's `click`/`hover` actions, which also
//! work in points. The marks are painted in **pixels** (points × scale) to
//! land on the right spot of the Retina capture. No stateful index cache is
//! needed: the index → center map travels in the same response.

use async_trait::async_trait;
use image::{Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryTreeParams};

use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::interactable::{collect_interactable, usable_bounds};
use super::types::DesktopOutput;

const SOM_DEFAULT_DEPTH: u32 = 16;
const SOM_MAX_DEPTH: u32 = 32;
const SOM_DEFAULT_ELEMENTS: usize = 100;
const SOM_MAX_ELEMENTS: usize = 300;
const SOM_TEXT_CAP: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DesktopSomArgs {
    /// Target process id. Omit to mark the frontmost application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// Maximum AX subtree depth to walk (default 16, capped at 32).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum number of elements to mark (default 100, capped at 300).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_elements: Option<usize>,
    /// Output image format: "png" (default) or "jpeg".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Max output width in pixels (downscale if wider). Marks are baked in
    /// before downscaling, so they scale with the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
}

/// A single element to mark, in **screen points** (pre-scale).
#[derive(Debug, Clone, Copy)]
struct Mark {
    index: usize,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

// ── 3×5 bitmap digit font ─────────────────────────────────────────────────────
//
// Each digit is five rows; the low three bits of every row are the columns
// (bit 2 = left). A pure const table keeps numbered labels legible without
// pulling in a font-rasterizer crate (R3 — core stays dependency-light).

const GLYPHS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b010, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

/// High-contrast palette cycled per mark so adjacent tags stay distinct.
const PALETTE: [Rgba<u8>; 6] = [
    Rgba([255, 59, 48, 255]),  // red
    Rgba([0, 122, 255, 255]),  // blue
    Rgba([52, 199, 89, 255]),  // green
    Rgba([255, 149, 0, 255]),  // orange
    Rgba([175, 82, 222, 255]), // purple
    Rgba([255, 45, 146, 255]), // pink
];

const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

/// Paint a filled rectangle, clamped to the image bounds.
fn fill_rect(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, color: Rgba<u8>) {
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(iw);
    let y1 = (y + h).min(ih);
    for py in y0..y1 {
        for px in x0..x1 {
            img.put_pixel(px as u32, py as u32, color);
        }
    }
}

/// Draw a rectangle outline of `thickness` px (clamped to the image).
fn draw_outline(img: &mut RgbaImage, x: i64, y: i64, w: i64, h: i64, t: i64, color: Rgba<u8>) {
    fill_rect(img, x, y, w, t, color); // top
    fill_rect(img, x, y + h - t, w, t, color); // bottom
    fill_rect(img, x, y, t, h, color); // left
    fill_rect(img, x + w - t, y, t, h, color); // right
}

/// Render one digit (0–9) at `(x, y)` with each font-pixel scaled to `gs²`.
fn draw_digit(img: &mut RgbaImage, x: i64, y: i64, digit: usize, gs: i64, color: Rgba<u8>) {
    let glyph = &GLYPHS[digit];
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..3 {
            if bits & (0b100 >> col) != 0 {
                fill_rect(img, x + col * gs, y + row as i64 * gs, gs, gs, color);
            }
        }
    }
}

/// Draw a numbered badge with its top-left at `(x, y)`. Returns the badge size
/// `(w, h)` so callers can keep it inside the image.
fn draw_label(img: &mut RgbaImage, x: i64, y: i64, n: usize, gs: i64, bg: Rgba<u8>) -> (i64, i64) {
    let digits: Vec<usize> = n.to_string().bytes().map(|b| (b - b'0') as usize).collect();
    let digit_w = 3 * gs;
    let digit_h = 5 * gs;
    let gap = gs;
    let pad = gs;
    let text_w = digits.len() as i64 * digit_w + (digits.len() as i64 - 1).max(0) * gap;
    let badge_w = text_w + 2 * pad;
    let badge_h = digit_h + 2 * pad;
    fill_rect(img, x, y, badge_w, badge_h, bg);
    let mut cx = x + pad;
    for d in digits {
        draw_digit(img, cx, y + pad, d, gs, WHITE);
        cx += digit_w + gap;
    }
    (badge_w, badge_h)
}

/// Choose a glyph scale proportional to image height so marks stay legible on
/// both small windows and large Retina captures.
fn glyph_scale(height: u32) -> i64 {
    ((height / 350) as i64).clamp(2, 6)
}

/// Paint numbered Set-of-Marks onto a PNG capture. Pure: decodes `png_bytes`,
/// scales each point-space `Mark` to pixels via `scale`, draws an outline plus
/// a numbered badge, and re-encodes to PNG. No platform access.
fn annotate(png_bytes: &[u8], marks: &[Mark], scale: f64) -> std::result::Result<Vec<u8>, String> {
    let mut img = image::load_from_memory(png_bytes)
        .map_err(|e| format!("decode capture: {e}"))?
        .to_rgba8();
    let gs = glyph_scale(img.height());
    let thickness = gs.max(2);
    let iw = img.width() as i64;
    let ih = img.height() as i64;

    for mark in marks {
        let color = PALETTE[mark.index % PALETTE.len()];
        let px = (mark.x * scale).round() as i64;
        let py = (mark.y * scale).round() as i64;
        let pw = (mark.w * scale).round() as i64;
        let ph = (mark.h * scale).round() as i64;
        draw_outline(&mut img, px, py, pw, ph, thickness, color);

        // Anchor the badge at the element's top-left, then nudge it back inside
        // the image so an element flush to an edge keeps a readable label.
        let badge_w_est = (mark.index.to_string().len() as i64) * (3 * gs + gs) + 2 * gs;
        let badge_h_est = 5 * gs + 2 * gs;
        let lx = px.clamp(0, (iw - badge_w_est).max(0));
        let ly = py.clamp(0, (ih - badge_h_est).max(0));
        draw_label(&mut img, lx, ly, mark.index, gs, color);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("encode annotated png: {e}"))?;
    Ok(buf.into_inner())
}

/// Char-safe truncation with an ellipsis marker.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let kept: String = s.chars().take(max_chars).collect();
        format!("{kept}…")
    } else {
        s.to_string()
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Build the `(marks, element-json)` pair for the collected elements. Centers
/// stay in screen points so they feed straight into `desktop` click actions.
fn build_marks(elements: &[&AxElement]) -> (Vec<Mark>, Vec<serde_json::Value>) {
    let mut marks = Vec::with_capacity(elements.len());
    let mut json = Vec::with_capacity(elements.len());
    for (index, el) in elements.iter().enumerate() {
        let (x, y, w, h) = match usable_bounds(el) {
            Some(b) => b,
            None => continue,
        };
        marks.push(Mark { index, x, y, w, h });
        let mut map = serde_json::Map::new();
        map.insert("index".into(), json!(index));
        map.insert("role".into(), json!(el.role));
        map.insert(
            "center".into(),
            json!([round1(x + w / 2.0), round1(y + h / 2.0)]),
        );
        if let Some(name) = el.title.as_deref().filter(|s| !s.trim().is_empty()) {
            map.insert("name".into(), json!(truncate(name, SOM_TEXT_CAP)));
        }
        if let Some(value) = el.value.as_deref().filter(|s| !s.trim().is_empty()) {
            map.insert("value".into(), json!(truncate(value, SOM_TEXT_CAP)));
        }
        json.push(serde_json::Value::Object(map));
    }
    (marks, json)
}

// ── Tool ──────────────────────────────────────────────────────────────────────

fn no_platform_output() -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(
            "Desktop platform capability is not configured for this server build.".to_string(),
        ),
    }
}

/// LLM tool: capture the screen with every actionable element numbered.
#[derive(Clone, Default)]
pub struct DesktopSom {
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
}

impl DesktopSom {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[async_trait]
impl AlephTool for DesktopSom {
    const NAME: &'static str = "desktop_som";
    const DESCRIPTION: &'static str =
        "Capture the screen with every clickable element outlined and numbered — a visual \
         Set-of-Marks for reliable GUI grounding. Returns the annotated image plus an \
         `elements` array where each entry has the on-image `index`, its accessibility \
         `role`/`name`, and a ready-to-use `center` [x, y]. Decide which numbered element \
         to act on from the image, then pass that element's `center` straight to the \
         `desktop` tool's `click`/`double_click`/`hover` actions. Prefer this over a plain \
         `screenshot` when you must point at something. Omit `pid` for the frontmost app. \
         macOS only — requires Accessibility + Screen Recording permission.";

    type Args = DesktopSomArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let platform = match self.platform.as_ref() {
            Some(p) => p,
            None => return Ok(no_platform_output()),
        };
        let ax = match platform.ax() {
            Some(a) => a,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(
                        "Accessibility (AX) capability is not available on this platform.".into(),
                    ),
                });
            }
        };
        let screen = match platform.screen() {
            Some(s) => s,
            None => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some("Screen capability is not available on this platform.".into()),
                });
            }
        };

        let max_depth = args
            .max_depth
            .unwrap_or(SOM_DEFAULT_DEPTH)
            .min(SOM_MAX_DEPTH);
        let max_elements = args
            .max_elements
            .unwrap_or(SOM_DEFAULT_ELEMENTS)
            .min(SOM_MAX_ELEMENTS);

        // 1. Collect actionable elements.
        let root = match ax
            .query_tree(QueryTreeParams {
                pid: args.pid,
                max_depth,
            })
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => {
                return Ok(DesktopOutput {
                    success: true,
                    data: Some(json!({ "count": 0, "elements": [] })),
                    message: Some("No accessible UI tree for the target application.".into()),
                });
            }
            Err(e) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("AX query failed: {e}")),
                });
            }
        };
        let mut collected: Vec<&AxElement> = Vec::new();
        collect_interactable(&root, &mut collected);
        let truncated = collected.len() > max_elements;
        collected.truncate(max_elements);
        let (marks, elements_json) = build_marks(&collected);

        // 2. Capture the screen (primary display; carries the Retina scale).
        let shot = match screen.screenshot(None).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Screen capture failed: {e}")),
                });
            }
        };
        let scale = shot.scale_factor.unwrap_or(1.0);

        // 3. Annotate off the async runtime (CPU-bound decode/draw/encode).
        use base64::Engine;
        let raw = match base64::engine::general_purpose::STANDARD.decode(&shot.image_base64) {
            Ok(b) => b,
            Err(e) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("capture base64 decode: {e}")),
                });
            }
        };
        let annotated =
            match tokio::task::spawn_blocking(move || annotate(&raw, &marks, scale)).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(e)) => {
                    return Ok(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("set-of-marks overlay failed: {e}")),
                    });
                }
                Err(e) => {
                    return Ok(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some(format!("overlay task join: {e}")),
                    });
                }
            };

        // 4. Honour format/max_width and the shared result-size budget — the
        //    same re-encoder the `screenshot` action uses.
        let out_fmt = args.format.clone().unwrap_or_else(|| "png".to_string());
        let max_w = args.max_width;
        let processed = match tokio::task::spawn_blocking(move || {
            aleph_desktop::perception::process_screenshot(
                &annotated,
                max_w,
                None,
                &out_fmt,
                90,
                Some(aleph_desktop::perception::DEFAULT_SCREENSHOT_MAX_BYTES),
            )
        })
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("annotated screenshot processing error: {e}")),
                });
            }
            Err(e) => {
                return Ok(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(format!("processing task join: {e}")),
                });
            }
        };

        Ok(DesktopOutput {
            success: true,
            data: Some(json!({
                "image_base64": processed.image_base64,
                "width": processed.width,
                "height": processed.height,
                "format": processed.format,
                "scale_factor": scale,
                "app_pid": root.pid,
                "count": elements_json.len(),
                "truncated": truncated,
                "elements": elements_json,
            })),
            message: None,
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::desktop_bridge::methods::screen::Region;

    fn leaf(role: &str, title: Option<&str>, bounds: Option<Region>) -> AxElement {
        AxElement {
            role: role.to_string(),
            title: title.map(str::to_string),
            value: None,
            bounds,
            pid: 1,
            children: Vec::new(),
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    /// Encode a solid-white RGBA image as PNG for annotation tests.
    fn white_png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, WHITE);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn glyph_scale_grows_with_height_and_is_clamped() {
        assert_eq!(glyph_scale(100), 2); // floor clamp
        assert_eq!(glyph_scale(700), 2);
        assert_eq!(glyph_scale(1400), 4);
        assert_eq!(glyph_scale(99999), 6); // ceiling clamp
    }

    #[test]
    fn build_marks_keeps_only_bounded_elements_with_point_centers() {
        let els = vec![
            leaf(
                "AXButton",
                Some("Save"),
                Some(rect(100.0, 200.0, 80.0, 40.0)),
            ),
            leaf("AXTextField", None, None), // no bounds → dropped
        ];
        let refs: Vec<&AxElement> = els.iter().collect();
        let (marks, json) = build_marks(&refs);
        assert_eq!(marks.len(), 1);
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["index"], 0);
        assert_eq!(json[0]["role"], "AXButton");
        assert_eq!(json[0]["name"], "Save");
        // center in points = top-left + size/2 = (140, 220).
        assert_eq!(json[0]["center"], json!([140.0, 220.0]));
    }

    #[test]
    fn annotate_paints_marks_and_keeps_dimensions() {
        let png = white_png(200, 120);
        let marks = vec![Mark {
            index: 12,
            x: 20.0,
            y: 30.0,
            w: 60.0,
            h: 24.0,
        }];
        let out = annotate(&png, &marks, 1.0).expect("annotate");
        let img = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(img.width(), 200);
        assert_eq!(img.height(), 120);
        // The all-white canvas must now contain coloured (non-white) pixels
        // from the outline/badge somewhere inside the element rect.
        let mut painted = 0u32;
        for py in 30..54 {
            for px in 20..80 {
                if img.get_pixel(px, py) != &WHITE {
                    painted += 1;
                }
            }
        }
        assert!(painted > 0, "expected painted overlay pixels");
    }

    #[test]
    fn annotate_scales_points_to_pixels() {
        // A 2x capture: a point-space mark at (10,10,10,10) lands at pixel
        // (20,20,20,20). The far corner of the painted outline must sit beyond
        // the un-scaled extent (>20px), proving the scale was applied.
        let png = white_png(100, 100);
        let marks = vec![Mark {
            index: 0,
            x: 10.0,
            y: 10.0,
            w: 10.0,
            h: 10.0,
        }];
        let out = annotate(&png, &marks, 2.0).expect("annotate");
        let img = image::load_from_memory(&out).unwrap().to_rgba8();
        // Right edge of the scaled outline is around x=38–39 (20 + 20 - t).
        let mut max_painted_x = 0u32;
        for py in 0..100 {
            for px in 0..100 {
                if img.get_pixel(px, py) != &WHITE {
                    max_painted_x = max_painted_x.max(px);
                }
            }
        }
        assert!(
            max_painted_x > 25,
            "scaled outline should extend past the un-scaled 20px extent, got {max_painted_x}"
        );
    }

    #[test]
    fn annotate_handles_out_of_bounds_marks_without_panicking() {
        let png = white_png(50, 50);
        let marks = vec![Mark {
            index: 99,
            x: 40.0,
            y: 40.0,
            w: 100.0, // extends well past the 50px image
            h: 100.0,
        }];
        let out = annotate(&png, &marks, 1.0).expect("annotate must clamp, not panic");
        let img = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(img.width(), 50);
    }

    #[tokio::test]
    async fn som_without_platform_returns_message() {
        let tool = DesktopSom::new();
        let out = tool
            .call(DesktopSomArgs {
                pid: None,
                max_depth: None,
                max_elements: None,
                format: None,
                max_width: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.is_some());
    }
}

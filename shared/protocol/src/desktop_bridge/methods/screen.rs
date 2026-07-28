use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CAPTURE: &str = "screen.capture";
pub const METHOD_OCR: &str = "screen.ocr";
pub const METHOD_LIST_DISPLAYS: &str = "screen.list_displays";

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for a screen capture.
///
/// Deliberately not the two seconds this constant used to name. `ScreenCaptureKit`
/// pays a real warm-up on the first capture after helper start (`SCShareableContent`
/// enumeration), and encoding a 6K display to PNG is not free either — a two-second
/// cap would turn a slow-but-working capture into a failure. What it is for is the
/// wedged case, and there the payoff is large: the macOS limb falls back to `xcap`
/// on error, so this number is how long a stuck helper delays a capture that would
/// otherwise have succeeded instantly on the other transport.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// Deadline for [`METHOD_OCR`]. Vision's accurate recognizer over a full-display
/// screenshot is seconds of real work, and there is no fallback transport.
pub const TIMEOUT_MS_OCR: u64 = 20_000;

/// Deadline for [`METHOD_LIST_DISPLAYS`] — an enumeration, not a capture.
pub const TIMEOUT_MS_LIST_DISPLAYS: u64 = 5_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] = &[
    (METHOD_OCR, TIMEOUT_MS_OCR),
    (METHOD_LIST_DISPLAYS, TIMEOUT_MS_LIST_DISPLAYS),
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CaptureParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Capture just this window (id from `window.list`), cropped to its frame,
    /// instead of a whole display. The window does not need to be frontmost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
    /// Draw the mouse cursor into the capture. Absent ⇒ platform default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_cursor: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CaptureResult {
    pub png_base64: String,
    /// Image width in PIXELS (`width_points * scale`).
    pub width: u32,
    /// Image height in PIXELS (`height_points * scale`).
    pub height: u32,
    /// Origin+size of the captured surface in the GLOBAL screen POINT space,
    /// top-left origin — i.e. the same space clicks are issued in.
    ///
    /// LOAD-BEARING for `window_id` captures: without it the crop's pixels
    /// cannot be mapped back to global click coordinates
    /// (`global = window_bounds.origin + pixel / scale`), and a window-cropped
    /// screenshot silently becomes a targeting regression. Absent on a
    /// full-display capture, and absent from a helper that predates the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_bounds: Option<Region>,
    /// Backing scale of the captured surface (2.0 on Retina): `pixels = points * scale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrParams {
    pub image_base64: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub fast_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrResult {
    pub full_text: String,
    pub blocks: Vec<OcrBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OcrBlock {
    pub text: String,
    pub bbox: Region,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDisplaysResult {
    pub displays: Vec<DisplayInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DisplayInfo {
    pub id: u32,
    pub bounds: Region,
    pub scale: f64,
    pub primary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_params_roundtrip() {
        let p = CaptureParams {
            display_id: Some(1),
            region: None,
            window_id: None,
            show_cursor: None,
        };
        let j = serde_json::to_string(&p).unwrap();
        // The pre-window-capture wire stays byte-identical when the new knobs
        // are unused, so an older helper sees exactly what it saw before.
        assert_eq!(j, r#"{"display_id":1}"#);
        let back: CaptureParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.display_id, Some(1));
        assert!(back.region.is_none());
        assert!(back.window_id.is_none());
    }

    #[test]
    fn capture_params_window_crop() {
        let j = r#"{"window_id":77,"show_cursor":false}"#;
        let p: CaptureParams = serde_json::from_str(j).unwrap();
        assert_eq!(p.window_id, Some(77));
        assert_eq!(p.show_cursor, Some(false));
    }

    #[test]
    fn capture_result_carries_the_mapping_back_to_global_points() {
        let j = r#"{"png_base64":"x","width":800,"height":600,
                    "window_bounds":{"x":100.0,"y":50.0,"width":400.0,"height":300.0},
                    "scale":2.0}"#;
        let r: CaptureResult = serde_json::from_str(j).unwrap();
        let b = r.window_bounds.expect("window_bounds");
        assert_eq!(b.x, 100.0);
        assert_eq!(r.scale, Some(2.0));
        // A pixel at (200, 100) in the crop is at global point (200, 100).
        let scale = r.scale.unwrap();
        assert_eq!(b.x + 200.0 / scale, 200.0);
        assert_eq!(b.y + 100.0 / scale, 100.0);
    }

    #[test]
    fn capture_result_without_window_bounds_stays_deserializable() {
        // Full-display capture, and any helper that predates the fields.
        let r: CaptureResult =
            serde_json::from_str(r#"{"png_base64":"x","width":10,"height":10}"#).unwrap();
        assert!(r.window_bounds.is_none());
        assert!(r.scale.is_none());
    }

    #[test]
    fn ocr_result_roundtrip() {
        let r = OcrResult {
            full_text: "hello".into(),
            blocks: vec![OcrBlock {
                text: "hello".into(),
                bbox: Region {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 20.0,
                },
                confidence: 0.99,
            }],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: OcrResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.full_text, "hello");
        assert_eq!(back.blocks.len(), 1);
        assert_eq!(back.blocks[0].text, "hello");
    }

    #[test]
    fn display_info_roundtrip() {
        let d = DisplayInfo {
            id: 1,
            bounds: Region {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            scale: 2.0,
            primary: true,
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: DisplayInfo = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, 1);
        assert!(back.primary);
        assert_eq!(back.scale, 2.0);
    }
}

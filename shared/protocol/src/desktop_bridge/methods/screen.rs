use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CAPTURE: &str = "screen.capture";
pub const METHOD_OCR: &str = "screen.ocr";
pub const METHOD_LIST_DISPLAYS: &str = "screen.list_displays";
pub const SUGGESTED_TIMEOUT_MS_CAPTURE: u64 = 2_000;
pub const SUGGESTED_TIMEOUT_MS_OCR: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CaptureParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
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
    pub width: u32,
    pub height: u32,
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
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: CaptureParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.display_id, Some(1));
        assert!(back.region.is_none());
    }

    #[test]
    fn ocr_result_roundtrip() {
        let r = OcrResult {
            full_text: "hello".into(),
            blocks: vec![OcrBlock {
                text: "hello".into(),
                bbox: Region { x: 0.0, y: 0.0, width: 100.0, height: 20.0 },
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
            bounds: Region { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 },
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

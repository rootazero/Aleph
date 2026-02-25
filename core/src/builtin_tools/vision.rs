//! Vision tool — image understanding and OCR via the [`VisionPipeline`].
//!
//! Wraps [`VisionPipeline`] behind the [`AlephTool`] interface so the AI agent
//! can describe images, answer visual questions, and extract text via OCR.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::error::Result;
use crate::tools::AlephTool;
use crate::vision::types::{ImageFormat, ImageInput};
use crate::vision::VisionPipeline;

// =============================================================================
// VisionAction — the set of operations the tool exposes
// =============================================================================

/// The action to perform with the vision tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisionAction {
    /// Describe an image or answer a question about its visual content.
    Understand,
    /// Extract text from an image via OCR.
    Ocr,
}

// =============================================================================
// VisionArgs — tool input
// =============================================================================

/// Arguments for the vision tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisionArgs {
    /// The vision action to perform.
    pub action: VisionAction,

    /// Base64-encoded image data (no data-URI prefix).
    ///
    /// Required for both `understand` and `ocr` actions.
    pub image_base64: String,

    /// Image format of the base64-encoded data.
    ///
    /// Defaults to `png` if omitted.
    #[serde(default = "default_format")]
    pub format: ImageFormat,

    /// Natural-language prompt describing what to look for or answer.
    ///
    /// Required for `understand`, ignored for `ocr`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

fn default_format() -> ImageFormat {
    ImageFormat::Png
}

// =============================================================================
// VisionOutput — tool output
// =============================================================================

/// Output from vision operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionOutput {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Human-readable message (present on errors or informational results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Structured data returned by the operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl VisionOutput {
    fn ok_data(message: impl Into<String>, data: Value) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: Some(data),
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: None,
        }
    }
}

// =============================================================================
// VisionTool
// =============================================================================

/// Vision tool — gives the AI agent image understanding and OCR capabilities.
///
/// Delegates to the [`VisionPipeline`] which orchestrates one or more providers
/// (Claude Vision, Platform OCR, etc.) in a fallback chain.
#[derive(Clone)]
pub struct VisionTool {
    pipeline: Arc<VisionPipeline>,
}

impl VisionTool {
    /// Create a new vision tool backed by the given pipeline.
    pub fn new(pipeline: Arc<VisionPipeline>) -> Self {
        Self { pipeline }
    }
}

// =============================================================================
// AlephTool implementation
// =============================================================================

#[async_trait]
impl AlephTool for VisionTool {
    const NAME: &'static str = "vision";

    const DESCRIPTION: &'static str = r#"Understand images and extract text via OCR.

Actions:
- understand: Describe an image or answer a visual question. Requires image_base64 and prompt.
- ocr: Extract text from an image. Requires image_base64.

Parameters:
- action: "understand" or "ocr"
- image_base64: Base64-encoded image data (no data-URI prefix)
- format: Image format — "png" (default), "jpeg", or "webp"
- prompt: Natural-language question about the image (required for "understand", ignored for "ocr")

Examples:
{"action":"understand","image_base64":"iVBORw0...","prompt":"What is shown in this image?"}
{"action":"understand","image_base64":"iVBORw0...","format":"jpeg","prompt":"Read all text visible in this screenshot"}
{"action":"ocr","image_base64":"iVBORw0..."}"#;

    type Args = VisionArgs;
    type Output = VisionOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"vision(action="understand", image_base64="...", prompt="What is this?") — describe an image"#.to_string(),
            r#"vision(action="understand", image_base64="...", prompt="Read all text") — read text using multimodal LLM"#.to_string(),
            r#"vision(action="ocr", image_base64="...") — extract text via OCR engine"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let image = ImageInput::Base64 {
            data: args.image_base64,
            format: args.format,
        };

        match args.action {
            VisionAction::Understand => {
                let prompt = match args.prompt {
                    Some(p) if !p.is_empty() => p,
                    _ => {
                        return Ok(VisionOutput::err(
                            "The 'understand' action requires a non-empty 'prompt' parameter.",
                        ))
                    }
                };

                match self.pipeline.understand_image(&image, &prompt).await {
                    Ok(result) => {
                        let data = serde_json::to_value(&result).unwrap_or_default();
                        Ok(VisionOutput::ok_data("Image understood", data))
                    }
                    Err(e) => Ok(VisionOutput::err(format!("Vision understand failed: {e}"))),
                }
            }
            VisionAction::Ocr => match self.pipeline.ocr(&image).await {
                Ok(result) => {
                    let data = serde_json::to_value(&result).unwrap_or_default();
                    Ok(VisionOutput::ok_data("OCR completed", data))
                }
                Err(e) => Ok(VisionOutput::err(format!("OCR failed: {e}"))),
            },
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::types::{OcrLine, OcrResult, Rect, VisionCapabilities, VisionResult};
    use crate::vision::{VisionError, VisionProvider};

    // ---- Mock provider -------------------------------------------------------

    #[derive(Clone)]
    struct MockVisionProvider;

    #[async_trait]
    impl VisionProvider for MockVisionProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            prompt: &str,
        ) -> std::result::Result<VisionResult, VisionError> {
            Ok(VisionResult {
                description: format!("Mock: {prompt}"),
                elements: vec![],
                confidence: 0.9,
            })
        }

        async fn ocr(
            &self,
            _image: &ImageInput,
        ) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "Mock OCR text".to_string(),
                lines: vec![OcrLine {
                    text: "Mock OCR text".to_string(),
                    bounding_box: Some(Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 20.0,
                    }),
                    confidence: 0.99,
                }],
            })
        }

        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities::all()
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    // ---- Helpers -------------------------------------------------------------

    fn make_tool() -> VisionTool {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(MockVisionProvider));
        VisionTool::new(Arc::new(pipeline))
    }

    fn make_args(action: VisionAction) -> VisionArgs {
        VisionArgs {
            action,
            image_base64: "iVBORw0KGgo=".to_string(),
            format: ImageFormat::Png,
            prompt: None,
        }
    }

    // ---- Tests ---------------------------------------------------------------

    #[test]
    fn test_tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "vision");
        assert!(def.description.contains("OCR"));
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_understand_success() {
        let tool = make_tool();
        let mut args = make_args(VisionAction::Understand);
        args.prompt = Some("Describe this image".to_string());

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.message.as_deref(), Some("Image understood"));
        let data = output.data.unwrap();
        assert!(data["description"].as_str().unwrap().contains("Mock"));
    }

    #[tokio::test]
    async fn test_understand_missing_prompt() {
        let tool = make_tool();
        let args = make_args(VisionAction::Understand);

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.unwrap().contains("prompt"));
    }

    #[tokio::test]
    async fn test_understand_empty_prompt() {
        let tool = make_tool();
        let mut args = make_args(VisionAction::Understand);
        args.prompt = Some("".to_string());

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.unwrap().contains("prompt"));
    }

    #[tokio::test]
    async fn test_ocr_success() {
        let tool = make_tool();
        let args = make_args(VisionAction::Ocr);

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.message.as_deref(), Some("OCR completed"));
        let data = output.data.unwrap();
        assert_eq!(data["full_text"], "Mock OCR text");
    }

    #[tokio::test]
    async fn test_empty_pipeline_returns_error() {
        let pipeline = VisionPipeline::new();
        let tool = VisionTool::new(Arc::new(pipeline));
        let mut args = make_args(VisionAction::Understand);
        args.prompt = Some("describe".to_string());

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.unwrap().contains("failed"));
    }

    #[test]
    fn test_vision_action_serde() {
        let action: VisionAction = serde_json::from_str(r#""understand""#).unwrap();
        assert!(matches!(action, VisionAction::Understand));

        let action: VisionAction = serde_json::from_str(r#""ocr""#).unwrap();
        assert!(matches!(action, VisionAction::Ocr));
    }

    #[test]
    fn test_vision_args_deserialization() {
        let json = serde_json::json!({
            "action": "understand",
            "image_base64": "abc123",
            "prompt": "What is this?"
        });
        let args: VisionArgs = serde_json::from_value(json).unwrap();
        assert!(matches!(args.action, VisionAction::Understand));
        assert_eq!(args.image_base64, "abc123");
        assert_eq!(args.prompt.as_deref(), Some("What is this?"));
        // default format
        assert!(matches!(args.format, ImageFormat::Png));
    }

    #[test]
    fn test_vision_args_with_format() {
        let json = serde_json::json!({
            "action": "ocr",
            "image_base64": "abc123",
            "format": "jpeg"
        });
        let args: VisionArgs = serde_json::from_value(json).unwrap();
        assert!(matches!(args.format, ImageFormat::Jpeg));
    }
}

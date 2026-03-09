//! `media_understand` tool — unified media understanding entry point.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::error::Result;
use crate::media::detect::{detect_by_extension, detect_from_path};
use crate::media::{MediaInput, MediaPipeline, MediaType};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaUnderstandArgs {
    /// Path to a local media file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    /// URL to a remote media resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Base64-encoded media data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64_data: Option<String>,

    /// File extension hint (e.g., "png", "mp3") when using base64 or URL without extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_hint: Option<String>,

    /// Natural-language prompt (e.g., "Describe this image", "Summarize this document").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaUnderstandOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl MediaUnderstandOutput {
    fn ok(media_type: &str, data: Value) -> Self {
        Self {
            success: true,
            media_type: Some(media_type.into()),
            message: None,
            data: Some(data),
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            media_type: None,
            message: Some(msg.into()),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct MediaUnderstandTool {
    pipeline: Arc<MediaPipeline>,
}

impl MediaUnderstandTool {
    pub fn new(pipeline: Arc<MediaPipeline>) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl AlephTool for MediaUnderstandTool {
    const NAME: &'static str = "media_understand";
    const DESCRIPTION: &'static str = r#"Understand media content (images, audio, video, documents).

Auto-detects the media type and routes to the appropriate processor.
Provide exactly one of: file_path, url, or base64_data.

Parameters:
- file_path: Path to a local file
- url: URL to remote media
- base64_data: Base64-encoded data (requires format_hint)
- format_hint: File extension hint (e.g., "png", "mp3", "pdf")
- prompt: What to extract or describe (optional)

Examples:
{"file_path":"/tmp/photo.jpg","prompt":"Describe this image"}
{"file_path":"/tmp/meeting.mp3","prompt":"Transcribe this audio"}
{"file_path":"/tmp/report.pdf"}"#;

    type Args = MediaUnderstandArgs;
    type Output = MediaUnderstandOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // 1. Build MediaInput from args
        let (input, media_type) = match (&args.file_path, &args.url, &args.base64_data) {
            (Some(path), None, None) => {
                let path = PathBuf::from(path);
                let mt = match detect_from_path(&path) {
                    Ok(mt) => mt,
                    Err(e) => {
                        return Ok(MediaUnderstandOutput::err(format!(
                            "Format detection failed: {}",
                            e
                        )))
                    }
                };
                (MediaInput::FilePath { path }, mt)
            }
            (None, Some(url), None) => {
                // Try format_hint first, then extract extension from URL
                let ext_str = args.format_hint.as_deref().or_else(|| {
                    url.rsplit('/')
                        .next()
                        .and_then(|name| name.rsplit('.').next())
                        .filter(|ext| !ext.contains('?') && ext.len() < 10)
                });
                let mt = match ext_str {
                    Some(ext) => detect_by_extension(ext).unwrap_or(MediaType::Unknown),
                    None => MediaType::Unknown,
                };
                (MediaInput::Url { url: url.clone() }, mt)
            }
            (None, None, Some(data)) => {
                let mt = match args.format_hint.as_deref() {
                    Some(ext) => detect_by_extension(ext).unwrap_or(MediaType::Unknown),
                    None => MediaType::Unknown,
                };
                (
                    MediaInput::Base64 {
                        data: data.clone(),
                        media_type: mt.clone(),
                    },
                    mt,
                )
            }
            _ => {
                return Ok(MediaUnderstandOutput::err(
                    "Provide exactly one of: file_path, url, or base64_data",
                ));
            }
        };

        if matches!(media_type, MediaType::Unknown) {
            return Ok(MediaUnderstandOutput::err(
                "Cannot detect media format. Provide a format_hint parameter.",
            ));
        }

        // 2. Process through pipeline
        let category = media_type.category().to_string();
        match self
            .pipeline
            .process(&input, &media_type, args.prompt.as_deref())
            .await
        {
            Ok(output) => {
                let data = serde_json::to_value(&output).unwrap_or_default();
                Ok(MediaUnderstandOutput::ok(&category, data))
            }
            Err(e) => Ok(MediaUnderstandOutput::err(format!(
                "Media processing failed: {}",
                e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::processors::ImageMediaProvider;
    use crate::vision::types::*;
    use crate::vision::{VisionError, VisionPipeline, VisionProvider};

    struct MockVision;

    #[async_trait]
    impl VisionProvider for MockVision {
        async fn understand_image(
            &self,
            _: &ImageInput,
            prompt: &str,
        ) -> std::result::Result<VisionResult, VisionError> {
            Ok(VisionResult {
                description: format!("Saw: {}", prompt),
                elements: vec![],
                confidence: 0.9,
            })
        }

        async fn ocr(&self, _: &ImageInput) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "OCR text".into(),
                lines: vec![],
            })
        }

        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities::all()
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    fn make_tool() -> MediaUnderstandTool {
        let mut vp = VisionPipeline::new();
        vp.add_provider(Box::new(MockVision));
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(ImageMediaProvider::new(Arc::new(vp), 10)));
        MediaUnderstandTool::new(Arc::new(mp))
    }

    #[tokio::test]
    async fn no_input_returns_error() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None,
            url: None,
            base64_data: None,
            format_hint: None,
            prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("exactly one"));
    }

    #[tokio::test]
    async fn url_with_extension() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None,
            url: Some("https://example.com/photo.png".into()),
            base64_data: None,
            format_hint: None,
            prompt: Some("describe".into()),
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.media_type.as_deref(), Some("image"));
    }

    #[tokio::test]
    async fn base64_with_format_hint() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None,
            url: None,
            base64_data: Some("iVBORw0KGgo=".into()),
            format_hint: Some("png".into()),
            prompt: Some("what is this?".into()),
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn unknown_format_returns_error() {
        let tool = make_tool();
        let args = MediaUnderstandArgs {
            file_path: None,
            url: None,
            base64_data: Some("abc".into()),
            format_hint: None,
            prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("format"));
    }

    #[test]
    fn tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "media_understand");
        assert!(def.description.contains("media"));
    }
}

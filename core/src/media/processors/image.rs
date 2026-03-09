//! Image processor — bridges existing VisionPipeline into the media pipeline.

use async_trait::async_trait;

use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::*;
use crate::sync_primitives::Arc;
use crate::vision::types::{ImageFormat as VisionImageFormat, ImageInput};
use crate::vision::{VisionError, VisionPipeline};

/// Bridges the existing [`VisionPipeline`] into the unified [`MediaProvider`] interface.
///
/// Converts MediaInput → ImageInput, delegates to VisionPipeline, converts results back.
pub struct ImageMediaProvider {
    pipeline: Arc<VisionPipeline>,
    priority: u8,
}

impl ImageMediaProvider {
    pub fn new(pipeline: Arc<VisionPipeline>, priority: u8) -> Self {
        Self { pipeline, priority }
    }

    /// Convert media image format to vision image format (best effort).
    fn to_vision_format(fmt: &MediaImageFormat) -> VisionImageFormat {
        match fmt {
            MediaImageFormat::Png => VisionImageFormat::Png,
            MediaImageFormat::Jpeg => VisionImageFormat::Jpeg,
            MediaImageFormat::WebP => VisionImageFormat::WebP,
            // Formats not directly supported by VisionPipeline — default to PNG
            MediaImageFormat::Gif | MediaImageFormat::Svg | MediaImageFormat::Heic => {
                VisionImageFormat::Png
            }
        }
    }

    fn convert_input(input: &MediaInput, _media_type: &MediaType) -> Result<ImageInput, MediaError> {
        match input {
            MediaInput::FilePath { path } => Ok(ImageInput::FilePath { path: path.clone() }),
            MediaInput::Url { url } => Ok(ImageInput::Url { url: url.clone() }),
            MediaInput::Base64 { data, media_type } => {
                let format = match media_type {
                    MediaType::Image { format, .. } => Self::to_vision_format(format),
                    _ => VisionImageFormat::Png,
                };
                Ok(ImageInput::Base64 {
                    data: data.clone(),
                    format,
                })
            }
        }
    }
}

#[async_trait]
impl MediaProvider for ImageMediaProvider {
    fn name(&self) -> &str {
        "image-vision-bridge"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    fn supported_types(&self) -> Vec<MediaType> {
        vec![MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        }]
    }

    async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        let image_input = Self::convert_input(input, media_type)?;

        if let Some(prompt_text) = prompt {
            match self.pipeline.understand_image(&image_input, prompt_text).await {
                Ok(result) => Ok(MediaOutput::Description {
                    text: result.description,
                    confidence: result.confidence,
                }),
                Err(VisionError::NoProvider) => Err(MediaError::NoProvider {
                    media_type: "image".to_string(),
                }),
                Err(e) => Err(MediaError::ProviderError {
                    provider: "vision-pipeline".to_string(),
                    message: e.to_string(),
                }),
            }
        } else {
            match self.pipeline.ocr(&image_input).await {
                Ok(result) => Ok(MediaOutput::Text {
                    text: result.full_text,
                }),
                Err(VisionError::NoProvider) => Err(MediaError::NoProvider {
                    media_type: "image".to_string(),
                }),
                Err(e) => Err(MediaError::ProviderError {
                    provider: "vision-pipeline".to_string(),
                    message: e.to_string(),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::types::{OcrResult, VisionCapabilities, VisionResult};
    use crate::vision::VisionProvider;

    struct MockVisionProvider;

    #[async_trait]
    impl VisionProvider for MockVisionProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            prompt: &str,
        ) -> Result<VisionResult, VisionError> {
            Ok(VisionResult {
                description: format!("Vision: {}", prompt),
                elements: vec![],
                confidence: 0.95,
            })
        }

        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "OCR extracted text".into(),
                lines: vec![],
            })
        }

        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities::all()
        }

        fn name(&self) -> &str {
            "mock-vision"
        }
    }

    fn make_provider() -> ImageMediaProvider {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(MockVisionProvider));
        ImageMediaProvider::new(Arc::new(pipeline), 10)
    }

    #[tokio::test]
    async fn understand_with_prompt() {
        let p = make_provider();
        let input = MediaInput::Url {
            url: "https://example.com/img.png".into(),
        };
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        let result = p.process(&input, &mt, Some("what is this?")).await.unwrap();
        match result {
            MediaOutput::Description { text, confidence } => {
                assert!(text.contains("Vision: what is this?"));
                assert!((confidence - 0.95).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Description"),
        }
    }

    #[tokio::test]
    async fn ocr_without_prompt() {
        let p = make_provider();
        let input = MediaInput::Url {
            url: "https://example.com/img.png".into(),
        };
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "OCR extracted text"),
            _ => panic!("Expected Text"),
        }
    }

    #[test]
    fn supports_image_category() {
        let p = make_provider();
        let png = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        let jpeg = MediaType::Image {
            format: MediaImageFormat::Jpeg,
            width: Some(100),
            height: Some(100),
        };
        let audio = MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        };
        assert!(p.supports(&png));
        assert!(p.supports(&jpeg));
        assert!(!p.supports(&audio));
    }

    #[test]
    fn format_conversion() {
        assert!(matches!(
            ImageMediaProvider::to_vision_format(&MediaImageFormat::Png),
            VisionImageFormat::Png
        ));
        assert!(matches!(
            ImageMediaProvider::to_vision_format(&MediaImageFormat::Jpeg),
            VisionImageFormat::Jpeg
        ));
        assert!(matches!(
            ImageMediaProvider::to_vision_format(&MediaImageFormat::Gif),
            VisionImageFormat::Png
        ));
    }
}

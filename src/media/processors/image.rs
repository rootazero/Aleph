//! Image processor — bridges existing `VisionPipeline` into the media pipeline.

use async_trait::async_trait;

use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::{MediaImageFormat, MediaInput, MediaOutput, MediaType};
use crate::sync_primitives::Arc;
use crate::vision::types::{ImageFormat as VisionImageFormat, ImageInput};
use crate::vision::{VisionError, VisionPipeline};

/// Bridges the existing [`VisionPipeline`] into the unified [`MediaProvider`] interface.
///
/// Converts `MediaInput` → `ImageInput`, delegates to `VisionPipeline`, converts results back.
pub struct ImageMediaProvider {
    pipeline: Arc<VisionPipeline>,
    priority: u8,
}

impl ImageMediaProvider {
    #[must_use]
    pub const fn new(pipeline: Arc<VisionPipeline>, priority: u8) -> Self {
        Self { pipeline, priority }
    }

    /// Convert media image format to vision image format.
    ///
    /// Returns `None` for formats not supported by the vision pipeline (GIF, SVG, HEIC).
    const fn to_vision_format(fmt: &MediaImageFormat) -> Option<VisionImageFormat> {
        match fmt {
            MediaImageFormat::Png => Some(VisionImageFormat::Png),
            MediaImageFormat::Jpeg => Some(VisionImageFormat::Jpeg),
            MediaImageFormat::WebP => Some(VisionImageFormat::WebP),
            MediaImageFormat::Gif | MediaImageFormat::Svg | MediaImageFormat::Heic => None,
        }
    }

    fn convert_input(
        input: &MediaInput,
        _media_type: &MediaType,
    ) -> Result<ImageInput, MediaError> {
        match input {
            // rust-doctor-disable-next-line excessive-clone
            MediaInput::FilePath { path } => Ok(ImageInput::FilePath { path: path.clone() }),
            // rust-doctor-disable-next-line excessive-clone
            MediaInput::Url { url } => Ok(ImageInput::Url { url: url.clone() }),
            MediaInput::Base64 { data, media_type } => {
                let format = match media_type {
                    MediaType::Image { format, .. } => Self::to_vision_format(format)
                        .ok_or_else(|| MediaError::UnsupportedFormat(format!("{:?}", format)))?,
                    _ => VisionImageFormat::Png,
                };
                Ok(ImageInput::Base64 {
                    // rust-doctor-disable-next-line excessive-clone
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
            match self
                .pipeline
                .understand_image(&image_input, prompt_text)
                .await
            {
                Ok(result) => Ok(MediaOutput::Description {
                    text: result.description,
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
    use crate::media::types::AudioFormat;
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
        };
        let result = p.process(&input, &mt, Some("what is this?")).await.unwrap();
        match result {
            MediaOutput::Description { text } => {
                assert!(text.contains("Vision: what is this?"));
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
        };
        let jpeg = MediaType::Image {
            format: MediaImageFormat::Jpeg,
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
            Some(VisionImageFormat::Png)
        ));
        assert!(matches!(
            ImageMediaProvider::to_vision_format(&MediaImageFormat::Jpeg),
            Some(VisionImageFormat::Jpeg)
        ));
        assert!(ImageMediaProvider::to_vision_format(&MediaImageFormat::Gif).is_none());
    }
}

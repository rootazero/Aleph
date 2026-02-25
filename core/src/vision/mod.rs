//! Vision module — image understanding, OCR, and object detection.
//!
//! This module defines the [`VisionProvider`] trait and a [`VisionPipeline`]
//! that orchestrates one or more providers in a fallback chain.

pub mod error;
pub mod provider;
pub mod providers;
pub mod types;

pub use error::VisionError;
pub use provider::VisionProvider;
pub use types::{
    ImageFormat, ImageInput, OcrLine, OcrResult, Rect, VisionCapabilities, VisionResult,
    VisualElement,
};

/// Orchestrates multiple [`VisionProvider`] instances in a fallback chain.
///
/// Providers are tried in registration order. The first provider that succeeds
/// wins; if all providers fail the last error is returned.
pub struct VisionPipeline {
    providers: Vec<Box<dyn VisionProvider>>,
}

impl VisionPipeline {
    /// Create an empty pipeline with no providers.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a provider. Providers are tried in the order they are added.
    pub fn add_provider(&mut self, provider: Box<dyn VisionProvider>) {
        self.providers.push(provider);
    }

    /// Return the number of registered providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Describe / answer a question about the given image.
    ///
    /// Tries each provider in order. Returns the first successful result, or
    /// the last error if every provider fails. Returns [`VisionError::NoProvider`]
    /// when the pipeline is empty.
    pub async fn understand_image(
        &self,
        image: &ImageInput,
        prompt: &str,
    ) -> Result<VisionResult, VisionError> {
        if self.providers.is_empty() {
            return Err(VisionError::NoProvider);
        }

        let mut last_err = VisionError::NoProvider;
        for provider in &self.providers {
            if !provider.capabilities().image_understanding {
                continue;
            }
            match provider.understand_image(image, prompt).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "Vision provider failed for understand_image, trying next"
                    );
                    last_err = e;
                }
            }
        }

        Err(last_err)
    }

    /// Extract text from the given image via OCR.
    ///
    /// Tries each provider in order. Returns the first successful result, or
    /// the last error if every provider fails. Returns [`VisionError::NoProvider`]
    /// when the pipeline is empty.
    pub async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError> {
        if self.providers.is_empty() {
            return Err(VisionError::NoProvider);
        }

        let mut last_err = VisionError::NoProvider;
        for provider in &self.providers {
            if !provider.capabilities().ocr {
                continue;
            }
            match provider.ocr(image).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "Vision provider failed for OCR, trying next"
                    );
                    last_err = e;
                }
            }
        }

        Err(last_err)
    }

    /// Aggregate capabilities across all registered providers.
    pub fn capabilities(&self) -> VisionCapabilities {
        let mut caps = VisionCapabilities::none();
        for p in &self.providers {
            let pc = p.capabilities();
            caps.image_understanding |= pc.image_understanding;
            caps.ocr |= pc.ocr;
            caps.object_detection |= pc.object_detection;
        }
        caps
    }
}

impl Default for VisionPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    // -------------------------------------------------------------------------
    // Mock providers
    // -------------------------------------------------------------------------

    /// A mock provider that always succeeds.
    struct SuccessProvider {
        tag: &'static str,
        caps: VisionCapabilities,
    }

    #[async_trait]
    impl VisionProvider for SuccessProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            prompt: &str,
        ) -> Result<VisionResult, VisionError> {
            Ok(VisionResult {
                description: format!("[{}] {}", self.tag, prompt),
                elements: vec![],
                confidence: 0.95,
            })
        }

        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: format!("[{}] recognized text", self.tag),
                lines: vec![OcrLine {
                    text: "Hello World".to_string(),
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
            self.caps
        }

        fn name(&self) -> &str {
            self.tag
        }
    }

    /// A mock provider that always fails.
    struct FailProvider;

    #[async_trait]
    impl VisionProvider for FailProvider {
        async fn understand_image(
            &self,
            _image: &ImageInput,
            _prompt: &str,
        ) -> Result<VisionResult, VisionError> {
            Err(VisionError::ProviderError("mock failure".into()))
        }

        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            Err(VisionError::ProviderError("mock failure".into()))
        }

        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities::all()
        }

        fn name(&self) -> &str {
            "fail-provider"
        }
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn sample_image() -> ImageInput {
        ImageInput::Base64 {
            data: "iVBORw0KGgo=".to_string(),
            format: ImageFormat::Png,
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn empty_pipeline_returns_no_provider() {
        let pipeline = VisionPipeline::new();
        let err = pipeline
            .understand_image(&sample_image(), "describe")
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::NoProvider));

        let err = pipeline.ocr(&sample_image()).await.unwrap_err();
        assert!(matches!(err, VisionError::NoProvider));
    }

    #[tokio::test]
    async fn single_success_provider() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "mock",
            caps: VisionCapabilities::all(),
        }));

        let result = pipeline
            .understand_image(&sample_image(), "what is this?")
            .await
            .unwrap();
        assert!(result.description.contains("[mock]"));
        assert!(result.description.contains("what is this?"));
        assert!((result.confidence - 0.95).abs() < f64::EPSILON);

        let ocr = pipeline.ocr(&sample_image()).await.unwrap();
        assert!(ocr.full_text.contains("[mock]"));
        assert_eq!(ocr.lines.len(), 1);
        assert_eq!(ocr.lines[0].text, "Hello World");
    }

    #[tokio::test]
    async fn fallback_on_failure() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(FailProvider));
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "backup",
            caps: VisionCapabilities::all(),
        }));

        let result = pipeline
            .understand_image(&sample_image(), "test")
            .await
            .unwrap();
        assert!(result.description.contains("[backup]"));
    }

    #[tokio::test]
    async fn all_providers_fail_returns_last_error() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(FailProvider));

        let err = pipeline
            .understand_image(&sample_image(), "test")
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::ProviderError(_)));
    }

    #[tokio::test]
    async fn skips_providers_without_capability() {
        let mut pipeline = VisionPipeline::new();
        // Provider that only supports OCR, not image understanding
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "ocr-only",
            caps: VisionCapabilities {
                image_understanding: false,
                ocr: true,
                object_detection: false,
            },
        }));
        // Provider that supports image understanding
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "vision",
            caps: VisionCapabilities {
                image_understanding: true,
                ocr: false,
                object_detection: false,
            },
        }));

        // understand_image should skip the ocr-only provider
        let result = pipeline
            .understand_image(&sample_image(), "describe")
            .await
            .unwrap();
        assert!(result.description.contains("[vision]"));

        // ocr should use the ocr-only provider
        let ocr = pipeline.ocr(&sample_image()).await.unwrap();
        assert!(ocr.full_text.contains("[ocr-only]"));
    }

    #[test]
    fn aggregated_capabilities() {
        let mut pipeline = VisionPipeline::new();
        assert_eq!(pipeline.capabilities(), VisionCapabilities::none());

        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "a",
            caps: VisionCapabilities {
                image_understanding: true,
                ocr: false,
                object_detection: false,
            },
        }));
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "b",
            caps: VisionCapabilities {
                image_understanding: false,
                ocr: true,
                object_detection: false,
            },
        }));

        let caps = pipeline.capabilities();
        assert!(caps.image_understanding);
        assert!(caps.ocr);
        assert!(!caps.object_detection);
    }

    #[test]
    fn provider_count() {
        let mut pipeline = VisionPipeline::new();
        assert_eq!(pipeline.provider_count(), 0);

        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "a",
            caps: VisionCapabilities::all(),
        }));
        assert_eq!(pipeline.provider_count(), 1);
    }

    #[test]
    fn image_format_mime_and_extension() {
        assert_eq!(ImageFormat::Png.mime_type(), "image/png");
        assert_eq!(ImageFormat::Png.extension(), "png");
        assert_eq!(ImageFormat::Jpeg.mime_type(), "image/jpeg");
        assert_eq!(ImageFormat::Jpeg.extension(), "jpeg");
        assert_eq!(ImageFormat::WebP.mime_type(), "image/webp");
        assert_eq!(ImageFormat::WebP.extension(), "webp");
    }

    #[test]
    fn vision_capabilities_all_and_none() {
        let all = VisionCapabilities::all();
        assert!(all.image_understanding);
        assert!(all.ocr);
        assert!(all.object_detection);

        let none = VisionCapabilities::none();
        assert!(!none.image_understanding);
        assert!(!none.ocr);
        assert!(!none.object_detection);

        assert_eq!(VisionCapabilities::default(), VisionCapabilities::none());
    }

    #[test]
    fn types_serialization_round_trip() {
        // ImageInput::Base64
        let input = ImageInput::Base64 {
            data: "abc123".to_string(),
            format: ImageFormat::Jpeg,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "base64");
        assert_eq!(json["format"], "jpeg");
        let round_trip: ImageInput = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, ImageInput::Base64 { .. }));

        // ImageInput::Url
        let input = ImageInput::Url {
            url: "https://example.com/image.png".to_string(),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "url");
        let round_trip: ImageInput = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, ImageInput::Url { .. }));

        // VisionResult
        let result = VisionResult {
            description: "A cat on a mat".to_string(),
            elements: vec![VisualElement {
                label: "cat".to_string(),
                element_type: "animal".to_string(),
                bounds: Some(Rect {
                    x: 10.0,
                    y: 20.0,
                    width: 100.0,
                    height: 80.0,
                }),
                confidence: 0.92,
            }],
            confidence: 0.95,
        };
        let json = serde_json::to_value(&result).unwrap();
        let round_trip: VisionResult = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.description, "A cat on a mat");
        assert_eq!(round_trip.elements.len(), 1);
        assert_eq!(round_trip.elements[0].label, "cat");

        // OcrResult
        let ocr = OcrResult {
            full_text: "Hello World".to_string(),
            lines: vec![OcrLine {
                text: "Hello World".to_string(),
                bounding_box: None,
                confidence: 0.98,
            }],
        };
        let json = serde_json::to_value(&ocr).unwrap();
        let round_trip: OcrResult = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.full_text, "Hello World");
        assert_eq!(round_trip.lines.len(), 1);
    }
}

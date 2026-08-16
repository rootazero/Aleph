//! Vision module — image understanding and OCR.
//!
//! This module defines the [`VisionProvider`] trait and a [`VisionPipeline`]
//! that orchestrates one or more providers in a fallback chain.

pub mod error;
pub mod provider;
pub mod providers;
pub mod types;

pub use error::VisionError;
pub use provider::VisionProvider;
pub use types::{ImageFormat, ImageInput, OcrResult, VisionCapabilities, VisionResult};

/// Orchestrates multiple [`VisionProvider`] instances in a fallback chain.
///
/// Providers are tried in registration order. The first provider that succeeds
/// wins; if all providers fail the last error is returned.
pub struct VisionPipeline {
    providers: Vec<Box<dyn VisionProvider>>,
}

impl VisionPipeline {
    /// Create an empty pipeline with no providers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a provider. Providers are tried in the order they are added.
    pub fn add_provider(&mut self, provider: Box<dyn VisionProvider>) {
        self.providers.push(provider);
    }

    /// Describe / answer a question about the given image.
    ///
    /// Tries each provider in order. Returns the first successful result, or
    /// the last error if every provider fails. Returns [`VisionError::NoProvider`]
    /// when the pipeline is empty, or [`VisionError::UnsupportedCapability`] when
    /// no registered provider supports image understanding.
    pub async fn understand_image(
        &self,
        image: &ImageInput,
        prompt: &str,
    ) -> Result<VisionResult, VisionError> {
        if self.providers.is_empty() {
            return Err(VisionError::NoProvider);
        }

        let mut last_err = VisionError::NoProvider;
        let mut any_capable = false;

        for provider in &self.providers {
            if !provider.capabilities().image_understanding {
                continue;
            }
            any_capable = true;
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

        if !any_capable {
            return Err(VisionError::UnsupportedCapability(
                "image_understanding".to_string(),
            ));
        }

        Err(last_err)
    }

    /// Extract text from the given image via OCR.
    ///
    /// Tries each provider in order. Returns the first successful result, or
    /// the last error if every provider fails. Returns [`VisionError::NoProvider`]
    /// when the pipeline is empty, or [`VisionError::UnsupportedCapability`] when
    /// no registered provider supports OCR.
    pub async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError> {
        if self.providers.is_empty() {
            return Err(VisionError::NoProvider);
        }

        let mut last_err = VisionError::NoProvider;
        let mut any_capable = false;

        for provider in &self.providers {
            if !provider.capabilities().ocr {
                continue;
            }
            any_capable = true;
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

        if !any_capable {
            return Err(VisionError::UnsupportedCapability("ocr".to_string()));
        }

        Err(last_err)
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
            })
        }

        async fn ocr(&self, _image: &ImageInput) -> Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: format!("[{}] recognized text", self.tag),
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

        let ocr = pipeline.ocr(&sample_image()).await.unwrap();
        assert!(ocr.full_text.contains("[mock]"));
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
    async fn no_provider_supports_capability_returns_unsupported() {
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "ocr-only",
            caps: VisionCapabilities {
                image_understanding: false,
                ocr: true,
            },
        }));

        let err = pipeline
            .understand_image(&sample_image(), "test")
            .await
            .unwrap_err();
        assert!(matches!(err, VisionError::UnsupportedCapability(_)));
        assert!(err.to_string().contains("image_understanding"));

        // Test OCR unsupported
        let mut pipeline = VisionPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "vision-only",
            caps: VisionCapabilities {
                image_understanding: true,
                ocr: false,
            },
        }));

        let err = pipeline.ocr(&sample_image()).await.unwrap_err();
        assert!(matches!(err, VisionError::UnsupportedCapability(_)));
        assert!(err.to_string().contains("ocr"));
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
            },
        }));
        // Provider that supports image understanding
        pipeline.add_provider(Box::new(SuccessProvider {
            tag: "vision",
            caps: VisionCapabilities {
                image_understanding: true,
                ocr: false,
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
    fn vision_capabilities_all_and_none() {
        let all = VisionCapabilities::all();
        assert!(all.image_understanding);
        assert!(all.ocr);

        let none = VisionCapabilities::none();
        assert!(!none.image_understanding);
        assert!(!none.ocr);

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

        // ImageInput::FilePath
        let input = ImageInput::FilePath {
            path: std::path::PathBuf::from("/tmp/test.png"),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["type"], "file_path");
        assert_eq!(json["path"], "/tmp/test.png");
        let round_trip: ImageInput = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, ImageInput::FilePath { .. }));

        // VisionResult
        let result = VisionResult {
            description: "A cat on a mat".to_string(),
        };
        let json = serde_json::to_value(&result).unwrap();
        let round_trip: VisionResult = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.description, "A cat on a mat");

        // OcrResult
        let ocr = OcrResult {
            full_text: "Hello World".to_string(),
        };
        let json = serde_json::to_value(&ocr).unwrap();
        let round_trip: OcrResult = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.full_text, "Hello World");
    }
}

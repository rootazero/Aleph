/// Media Generation Provider abstraction for Aether
///
/// This module defines the `GenerationProvider` trait which provides a unified interface
/// for different media generation backends (DALL-E, Stable Diffusion, Runway, ElevenLabs, etc.).
///
/// # Architecture
///
/// All generation providers implement the `GenerationProvider` trait, which defines:
/// - `generate()`: Async method to generate media from a request
/// - `name()`: Provider identifier
/// - `supported_types()`: List of generation types this provider supports
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::{GenerationProvider, GenerationRequest, GenerationType};
/// use std::sync::Arc;
///
/// async fn example(provider: Arc<dyn GenerationProvider>) {
///     let request = GenerationRequest::image("A sunset over mountains");
///     let output = provider.generate(request).await.unwrap();
///
///     println!("Provider: {}", provider.name());
///     println!("Generated: {:?}", output.data);
/// }
/// ```
///
/// # Supported Generation Types
///
/// - `Image`: Static images (DALL-E, Stable Diffusion, Midjourney)
/// - `Video`: Video clips (Runway, Pika, Sora)
/// - `Audio`: Music and sound (Suno, Udio)
/// - `Speech`: Text-to-speech (ElevenLabs, OpenAI TTS)
use std::future::Future;
use std::pin::Pin;

// Sub-modules
pub mod error;
pub mod providers;
pub mod registry;
pub mod response_parser;
pub mod types;

// Re-exports
pub use error::{GenerationError, GenerationResult};
pub use registry::GenerationProviderRegistry;
pub use response_parser::{
    has_generation_requests, parse_generation_requests, ParsedGenerationRequest, ParseResult,
};
pub use types::{
    GenerationData, GenerationMetadata, GenerationOutput, GenerationParams,
    GenerationParamsBuilder, GenerationProgress, GenerationRequest, GenerationType,
};

/// Unified interface for media generation providers
///
/// All media generation backends (DALL-E, Stable Diffusion, Runway, ElevenLabs, etc.)
/// implement this trait to provide a consistent API for generating media content.
///
/// # Thread Safety
///
/// The trait extends `Send + Sync` to ensure providers can be safely shared
/// across async tasks and stored in `Arc<dyn GenerationProvider>`.
///
/// # Async Design
///
/// All generation operations are async to avoid blocking the runtime during
/// API calls or long-running generation jobs.
///
/// # Example Implementation
///
/// ```rust,ignore
/// use aethecore::generation::{
///     GenerationProvider, GenerationRequest, GenerationOutput, GenerationResult,
///     GenerationType, GenerationData,
/// };
/// use std::future::Future;
/// use std::pin::Pin;
///
/// struct MyProvider {
///     name: String,
/// }
///
/// impl GenerationProvider for MyProvider {
///     fn generate(
///         &self,
///         request: GenerationRequest,
///     ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
///         Box::pin(async move {
///             // Perform generation...
///             Ok(GenerationOutput::new(
///                 request.generation_type,
///                 GenerationData::url("https://example.com/image.png"),
///             ))
///         })
///     }
///
///     fn name(&self) -> &str {
///         &self.name
///     }
///
///     fn supported_types(&self) -> Vec<GenerationType> {
///         vec![GenerationType::Image]
///     }
/// }
/// ```
pub trait GenerationProvider: Send + Sync {
    /// Generate media from a request
    ///
    /// # Arguments
    ///
    /// * `request` - The generation request containing prompt, type, and parameters
    ///
    /// # Returns
    ///
    /// * `Ok(GenerationOutput)` - The generated media with metadata
    /// * `Err(GenerationError)` - Various errors:
    ///   - `AuthenticationError`: Invalid API key
    ///   - `RateLimitError`: Too many requests
    ///   - `ContentFilteredError`: Content blocked by safety filters
    ///   - `TimeoutError`: Generation took too long
    ///   - `ProviderError`: Provider returned an error
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::{GenerationProvider, GenerationRequest};
    ///
    /// async fn generate_image(provider: &dyn GenerationProvider) {
    ///     let request = GenerationRequest::image("A cat wearing a hat");
    ///     let output = provider.generate(request).await.unwrap();
    ///     println!("Generated: {:?}", output.data);
    /// }
    /// ```
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>>;

    /// Get provider name for logging and routing
    ///
    /// # Returns
    ///
    /// Provider identifier (e.g., "dalle", "stable-diffusion", "elevenlabs")
    fn name(&self) -> &str;

    /// Get the list of generation types this provider supports
    ///
    /// # Returns
    ///
    /// Vector of supported generation types
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::{GenerationProvider, GenerationType};
    ///
    /// fn check_support(provider: &dyn GenerationProvider) {
    ///     let types = provider.supported_types();
    ///     if types.contains(&GenerationType::Image) {
    ///         println!("{} supports image generation", provider.name());
    ///     }
    /// }
    /// ```
    fn supported_types(&self) -> Vec<GenerationType>;

    /// Check if this provider supports a specific generation type
    ///
    /// # Arguments
    ///
    /// * `gen_type` - The generation type to check
    ///
    /// # Returns
    ///
    /// `true` if the provider supports this type, `false` otherwise
    ///
    /// # Default Implementation
    ///
    /// Checks if `gen_type` is in `supported_types()`.
    fn supports(&self, gen_type: GenerationType) -> bool {
        self.supported_types().contains(&gen_type)
    }

    /// Get provider brand color for UI theming (optional)
    ///
    /// # Returns
    ///
    /// Hex color string (e.g., "#10a37f" for OpenAI green)
    ///
    /// # Default Implementation
    ///
    /// Returns a default gray color "#808080".
    fn color(&self) -> &str {
        "#808080"
    }

    /// Get the default model for this provider (optional)
    ///
    /// # Returns
    ///
    /// The default model name, or `None` if not applicable
    ///
    /// # Default Implementation
    ///
    /// Returns `None`.
    fn default_model(&self) -> Option<&str> {
        None
    }

    /// Check generation progress for long-running operations (optional)
    ///
    /// Some providers (especially video/audio) use async job polling.
    /// This method allows checking the progress of such operations.
    ///
    /// # Arguments
    ///
    /// * `job_id` - The job ID returned from an initial generate call
    ///
    /// # Returns
    ///
    /// * `Ok(GenerationProgress)` - Current progress information
    /// * `Err(GenerationError)` - Error checking progress
    ///
    /// # Default Implementation
    ///
    /// Returns an error indicating the feature is not supported.
    fn check_progress(
        &self,
        _job_id: &str,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationProgress>> + Send + '_>> {
        Box::pin(async {
            Err(GenerationError::unsupported_feature(
                "Progress checking is not supported by this provider",
                "check_progress",
                self.name(),
            ))
        })
    }

    /// Cancel a generation job (optional)
    ///
    /// # Arguments
    ///
    /// * `job_id` - The job ID to cancel
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Job cancelled successfully
    /// * `Err(GenerationError)` - Error cancelling job
    ///
    /// # Default Implementation
    ///
    /// Returns an error indicating the feature is not supported.
    fn cancel(
        &self,
        _job_id: &str,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<()>> + Send + '_>> {
        Box::pin(async {
            Err(GenerationError::unsupported_feature(
                "Cancellation is not supported by this provider",
                "cancel",
                self.name(),
            ))
        })
    }

    /// Edit an existing image using a prompt (optional)
    ///
    /// This method supports image-to-image generation where an input image
    /// is modified based on a text prompt. Some providers call this "inpainting"
    /// or "image editing".
    ///
    /// # Arguments
    ///
    /// * `request` - The edit request containing the prompt and parameters
    ///   - `params.reference_image`: Required - base64-encoded input image or URL
    ///   - `prompt`: The edit instructions
    ///   - `params.mask`: Optional - base64-encoded mask image (transparent areas = edit regions)
    ///
    /// # Returns
    ///
    /// * `Ok(GenerationOutput)` - The edited image
    /// * `Err(GenerationError)` - Various errors including UnsupportedFeatureError
    ///
    /// # Default Implementation
    ///
    /// Returns an error indicating the feature is not supported.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
    ///
    /// async fn edit_image(provider: &dyn GenerationProvider) {
    ///     let request = GenerationRequest::image("Add a hat to the person")
    ///         .with_params(
    ///             GenerationParams::builder()
    ///                 .reference_image("base64_encoded_image_data")
    ///                 .build()
    ///         );
    ///     let output = provider.edit_image(request).await.unwrap();
    ///     println!("Edited: {:?}", output.data);
    /// }
    /// ```
    fn edit_image(
        &self,
        _request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async {
            Err(GenerationError::unsupported_feature(
                "Image editing is not supported by this provider",
                "edit_image",
                self.name(),
            ))
        })
    }

    /// Check if this provider supports image editing
    ///
    /// # Returns
    ///
    /// `true` if the provider supports the `edit_image` method
    ///
    /// # Default Implementation
    ///
    /// Returns `false`.
    fn supports_image_editing(&self) -> bool {
        false
    }
}

/// Mock generation provider for testing
///
/// This provider returns predictable mock responses and can be configured
/// for different scenarios including success, errors, and delays.
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::{MockGenerationProvider, GenerationRequest, GenerationProvider};
///
/// # tokio_test::block_on(async {
/// let provider = MockGenerationProvider::new("mock-dalle");
///
/// let request = GenerationRequest::image("A test image");
/// let output = provider.generate(request).await.unwrap();
///
/// assert!(output.data.is_url());
/// # });
/// ```
pub struct MockGenerationProvider {
    name: String,
    color: String,
    supported_types: Vec<GenerationType>,
    should_fail: bool,
    error_message: Option<String>,
}

impl MockGenerationProvider {
    /// Create a new mock provider with the given name
    ///
    /// By default, supports Image and Speech generation.
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            color: "#808080".to_string(),
            supported_types: vec![GenerationType::Image, GenerationType::Speech],
            should_fail: false,
            error_message: None,
        }
    }

    /// Set the color for this mock provider
    pub fn with_color<S: Into<String>>(mut self, color: S) -> Self {
        self.color = color.into();
        self
    }

    /// Set the supported generation types
    pub fn with_types(mut self, types: Vec<GenerationType>) -> Self {
        self.supported_types = types;
        self
    }

    /// Configure the provider to fail with an error
    pub fn with_failure<S: Into<String>>(mut self, message: S) -> Self {
        self.should_fail = true;
        self.error_message = Some(message.into());
        self
    }

    /// Create a mock provider that supports all generation types
    pub fn all_types<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            color: "#808080".to_string(),
            supported_types: vec![
                GenerationType::Image,
                GenerationType::Video,
                GenerationType::Audio,
                GenerationType::Speech,
            ],
            should_fail: false,
            error_message: None,
        }
    }

    /// Create a mock provider for image generation only
    pub fn image_only<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            color: "#808080".to_string(),
            supported_types: vec![GenerationType::Image],
            should_fail: false,
            error_message: None,
        }
    }

    /// Create a mock provider for video generation only
    pub fn video_only<S: Into<String>>(name: S) -> Self {
        Self {
            name: name.into(),
            color: "#808080".to_string(),
            supported_types: vec![GenerationType::Video],
            should_fail: false,
            error_message: None,
        }
    }
}

impl GenerationProvider for MockGenerationProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        let name = self.name.clone();
        let should_fail = self.should_fail;
        let error_message = self.error_message.clone();

        Box::pin(async move {
            if should_fail {
                return Err(GenerationError::provider(
                    error_message.unwrap_or_else(|| "Mock error".to_string()),
                    Some(500),
                    name,
                ));
            }

            // Generate a mock URL based on the generation type
            let url = match request.generation_type {
                GenerationType::Image => {
                    format!("https://mock.example.com/{}/image.png", name)
                }
                GenerationType::Video => {
                    format!("https://mock.example.com/{}/video.mp4", name)
                }
                GenerationType::Audio => {
                    format!("https://mock.example.com/{}/audio.mp3", name)
                }
                GenerationType::Speech => {
                    format!("https://mock.example.com/{}/speech.mp3", name)
                }
            };

            let data = GenerationData::url(url);
            let metadata = GenerationMetadata::new()
                .with_provider(name.clone())
                .with_model("mock-model")
                .with_revised_prompt(format!("Mock processed: {}", request.prompt));

            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.supported_types.clone()
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some("mock-model")
    }
}

/// Create a mock generation provider for testing
///
/// Returns an `Arc<dyn GenerationProvider>` wrapping a MockGenerationProvider.
/// This is useful for testing services that require a GenerationProvider.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::create_mock_generation_provider;
///
/// let provider = create_mock_generation_provider();
/// assert_eq!(provider.name(), "mock");
/// ```
pub fn create_mock_generation_provider() -> std::sync::Arc<dyn GenerationProvider> {
    std::sync::Arc::new(MockGenerationProvider::new("mock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // === Trait object tests ===

    #[tokio::test]
    async fn test_provider_trait_object() {
        let provider: Arc<dyn GenerationProvider> = Arc::new(MockGenerationProvider::new("test"));

        // Test generate method
        let request = GenerationRequest::image("A test image");
        let output = provider.generate(request).await.unwrap();

        assert!(output.data.is_url());
        assert_eq!(output.generation_type, GenerationType::Image);

        // Test metadata methods
        assert_eq!(provider.name(), "test");
        assert_eq!(provider.color(), "#808080");
    }

    #[tokio::test]
    async fn test_provider_generate_different_types() {
        let provider = MockGenerationProvider::all_types("multi");

        // Image
        let image_request = GenerationRequest::image("An image");
        let image_output = provider.generate(image_request).await.unwrap();
        assert_eq!(image_output.generation_type, GenerationType::Image);
        assert!(image_output.data.as_url().unwrap().contains("image.png"));

        // Video
        let video_request = GenerationRequest::video("A video");
        let video_output = provider.generate(video_request).await.unwrap();
        assert_eq!(video_output.generation_type, GenerationType::Video);
        assert!(video_output.data.as_url().unwrap().contains("video.mp4"));

        // Audio
        let audio_request = GenerationRequest::audio("Some music");
        let audio_output = provider.generate(audio_request).await.unwrap();
        assert_eq!(audio_output.generation_type, GenerationType::Audio);
        assert!(audio_output.data.as_url().unwrap().contains("audio.mp3"));

        // Speech
        let speech_request = GenerationRequest::speech("Hello world");
        let speech_output = provider.generate(speech_request).await.unwrap();
        assert_eq!(speech_output.generation_type, GenerationType::Speech);
        assert!(speech_output.data.as_url().unwrap().contains("speech.mp3"));
    }

    #[test]
    fn test_provider_supports() {
        let image_only = MockGenerationProvider::image_only("dalle");

        assert!(image_only.supports(GenerationType::Image));
        assert!(!image_only.supports(GenerationType::Video));
        assert!(!image_only.supports(GenerationType::Audio));
        assert!(!image_only.supports(GenerationType::Speech));

        let all_types = MockGenerationProvider::all_types("all");

        assert!(all_types.supports(GenerationType::Image));
        assert!(all_types.supports(GenerationType::Video));
        assert!(all_types.supports(GenerationType::Audio));
        assert!(all_types.supports(GenerationType::Speech));
    }

    #[test]
    fn test_provider_supported_types() {
        let provider = MockGenerationProvider::new("test");

        let types = provider.supported_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&GenerationType::Image));
        assert!(types.contains(&GenerationType::Speech));
    }

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn GenerationProvider>>();
    }

    // === MockGenerationProvider tests ===

    #[test]
    fn test_mock_provider_new() {
        let provider = MockGenerationProvider::new("dalle");

        assert_eq!(provider.name(), "dalle");
        assert_eq!(provider.color(), "#808080");
        assert_eq!(provider.default_model(), Some("mock-model"));
    }

    #[test]
    fn test_mock_provider_with_color() {
        let provider = MockGenerationProvider::new("dalle").with_color("#FF0000");

        assert_eq!(provider.color(), "#FF0000");
    }

    #[test]
    fn test_mock_provider_with_types() {
        let provider = MockGenerationProvider::new("test")
            .with_types(vec![GenerationType::Video, GenerationType::Audio]);

        assert!(provider.supports(GenerationType::Video));
        assert!(provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Image));
    }

    #[tokio::test]
    async fn test_mock_provider_failure() {
        let provider = MockGenerationProvider::new("failing").with_failure("Test error message");

        let request = GenerationRequest::image("test");
        let result = provider.generate(request).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Test error message"));
    }

    #[tokio::test]
    async fn test_mock_provider_with_request_id() {
        let provider = MockGenerationProvider::new("test");

        let request = GenerationRequest::image("test").with_request_id("req-123");

        let output = provider.generate(request).await.unwrap();

        assert_eq!(output.request_id, Some("req-123".to_string()));
    }

    #[tokio::test]
    async fn test_mock_provider_metadata() {
        let provider = MockGenerationProvider::new("dalle");

        let request = GenerationRequest::image("A beautiful sunset");
        let output = provider.generate(request).await.unwrap();

        assert_eq!(output.metadata.provider, Some("dalle".to_string()));
        assert_eq!(output.metadata.model, Some("mock-model".to_string()));
        assert!(output.metadata.revised_prompt.is_some());
        assert!(output
            .metadata
            .revised_prompt
            .unwrap()
            .contains("A beautiful sunset"));
    }

    // === Default method tests ===

    #[tokio::test]
    async fn test_check_progress_not_supported() {
        let provider = MockGenerationProvider::new("test");

        let result = provider.check_progress("job-123").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::UnsupportedFeatureError { .. }
        ));
    }

    #[tokio::test]
    async fn test_cancel_not_supported() {
        let provider = MockGenerationProvider::new("test");

        let result = provider.cancel("job-123").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::UnsupportedFeatureError { .. }
        ));
    }

    // === Factory function tests ===

    #[test]
    fn test_create_mock_generation_provider() {
        let provider = create_mock_generation_provider();

        assert_eq!(provider.name(), "mock");
        assert!(provider.supports(GenerationType::Image));
    }

    #[tokio::test]
    async fn test_create_mock_generation_provider_works() {
        let provider = create_mock_generation_provider();

        let request = GenerationRequest::image("test");
        let output = provider.generate(request).await.unwrap();

        assert!(output.data.is_url());
    }

    // === Integration tests ===

    #[tokio::test]
    async fn test_full_generation_flow() {
        // Create provider
        let provider: Arc<dyn GenerationProvider> =
            Arc::new(MockGenerationProvider::new("dalle").with_color("#10a37f"));

        // Build request with params
        let params = GenerationParams::builder()
            .width(1024)
            .height(1024)
            .quality("hd")
            .style("vivid")
            .n(1)
            .build();

        let request = GenerationRequest::image("A majestic mountain landscape")
            .with_params(params)
            .with_request_id("req-001")
            .with_user_id("user-123");

        // Generate
        let output = provider.generate(request).await.unwrap();

        // Verify output
        assert_eq!(output.generation_type, GenerationType::Image);
        assert!(output.data.is_url());
        assert_eq!(output.request_id, Some("req-001".to_string()));
        assert_eq!(output.metadata.provider, Some("dalle".to_string()));

        // Verify provider info
        assert_eq!(provider.name(), "dalle");
        assert_eq!(provider.color(), "#10a37f");
        assert!(provider.supports(GenerationType::Image));
    }
}

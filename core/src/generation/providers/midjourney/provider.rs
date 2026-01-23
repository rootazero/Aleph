//! Core MidjourneyProvider struct and implementation
//!
//! Contains the main provider struct, constructor, URL generation methods,
//! and the GenerationProvider trait implementation.

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;
use tracing::info;

use super::builder::MidjourneyProviderBuilder;
use super::submit_polling::SubmitPolling;
use super::types::{ImagineRequest, MidjourneyMode, PROVIDER_NAME};

/// T8Star Midjourney Image Generation Provider
///
/// This provider integrates with T8Star's Midjourney API proxy to create
/// high-quality images from text prompts.
///
/// # Features
///
/// - Midjourney image generation from text prompts
/// - Fast and Relax mode support
/// - Automatic polling for async generation
/// - Base64 image input support (for image references)
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::{MidjourneyProvider, MidjourneyMode};
/// use aethecore::generation::GenerationProvider;
///
/// let provider = MidjourneyProvider::builder("your-api-key")
///     .mode(MidjourneyMode::Fast)
///     .build();
///
/// assert_eq!(provider.name(), "midjourney");
/// ```
#[derive(Debug, Clone)]
pub struct MidjourneyProvider {
    /// Provider name (typically "midjourney")
    pub(crate) name: String,
    /// HTTP client for making requests
    pub(crate) client: Client,
    /// API key for authentication
    pub(crate) api_key: String,
    /// API endpoint (e.g., "https://ai.t8star.cn")
    pub(crate) endpoint: String,
    /// Generation mode (Fast or Relax)
    pub(crate) mode: MidjourneyMode,
    /// Brand color for UI theming
    pub(crate) color: String,
}

impl MidjourneyProvider {
    /// Create a new MidjourneyProvider with default settings
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::providers::MidjourneyProvider;
    /// use aethecore::GenerationProvider; // Import trait for name() method
    ///
    /// let provider = MidjourneyProvider::new("your-api-key");
    /// assert_eq!(provider.name(), "midjourney");
    /// ```
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        MidjourneyProviderBuilder::new(api_key).build()
    }

    /// Create a builder for MidjourneyProvider
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::providers::{MidjourneyProvider, MidjourneyMode};
    ///
    /// let provider = MidjourneyProvider::builder("your-api-key")
    ///     .mode(MidjourneyMode::Relax)
    ///     .color("#FF0000")
    ///     .timeout_secs(60)
    ///     .build();
    /// ```
    pub fn builder<S: Into<String>>(api_key: S) -> MidjourneyProviderBuilder {
        MidjourneyProviderBuilder::new(api_key)
    }

    /// Get the full URL for the imagine submit endpoint
    pub(crate) fn submit_url(&self) -> String {
        format!(
            "{}/{}/mj/submit/imagine",
            self.endpoint,
            self.mode.as_path()
        )
    }

    /// Get the URL for fetching task status
    pub(crate) fn task_url(&self, task_id: &str) -> String {
        format!(
            "{}/{}/mj/task/{}/fetch",
            self.endpoint,
            self.mode.as_path(),
            task_id
        )
    }
}

// === GenerationProvider Implementation ===

impl GenerationProvider for MidjourneyProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    PROVIDER_NAME,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            info!(
                prompt = %request.prompt,
                mode = %self.mode,
                "Starting Midjourney image generation"
            );

            // Build imagine request
            let imagine_request = ImagineRequest {
                prompt: request.prompt.clone(),
                base64_array: request
                    .params
                    .reference_image
                    .as_ref()
                    .map(|img| vec![img.clone()]),
            };

            // Submit task
            let task_id = self.submit_imagine(&imagine_request).await?;

            // Poll for completion
            let task = self.poll_task(&task_id).await?;

            // Extract image URL
            let image_url = task.image_url.ok_or_else(|| {
                GenerationError::provider("No image URL in completed task", None, PROVIDER_NAME)
            })?;

            // Download image bytes
            let bytes = self.download_image(&image_url).await?;
            let data = GenerationData::bytes(bytes.clone());

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(PROVIDER_NAME)
                .with_model(PROVIDER_NAME)
                .with_duration(duration)
                .with_content_type("image/png")
                .with_size_bytes(bytes.len() as u64);

            // Add mode info
            metadata.extra.insert(
                "mode".to_string(),
                serde_json::Value::String(self.mode.to_string()),
            );
            metadata.extra.insert(
                "task_id".to_string(),
                serde_json::Value::String(task_id.clone()),
            );

            // Add buttons info if available
            if let Some(buttons) = &task.buttons {
                let button_labels: Vec<String> = buttons.iter().map(|b| b.label.clone()).collect();
                metadata.extra.insert(
                    "actions".to_string(),
                    serde_json::Value::Array(
                        button_labels
                            .iter()
                            .map(|l| serde_json::Value::String(l.clone()))
                            .collect(),
                    ),
                );
            }

            info!(
                duration_ms = duration.as_millis(),
                task_id = %task_id,
                mode = %self.mode,
                "Midjourney image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Image, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(PROVIDER_NAME)
    }
}

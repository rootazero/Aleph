//! ReplicateProvider implementation
//!
//! This module contains the main provider struct and its GenerationProvider trait implementation.

use super::builder::ReplicateProviderBuilder;
use super::input::build_input;
use super::prediction::{create_prediction, fetch_output, poll_prediction};
use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Replicate API Provider for media generation
///
/// This provider integrates with Replicate's prediction API to run various
/// AI models for image, audio, and video generation.
///
/// # Features
///
/// - Async polling-based generation
/// - Configurable model mappings (alias -> full version)
/// - Multiple generation types (Image, Audio)
/// - Automatic output fetching
///
/// # Example
///
/// ```rust
/// use alephcore::generation::providers::ReplicateProvider;
/// use alephcore::generation::{GenerationProvider, GenerationType};
///
/// let provider = ReplicateProvider::builder("r8_your_api_token")
///     .add_model("flux", "black-forest-labs/flux-schnell")
///     .supported_types(vec![GenerationType::Image, GenerationType::Audio])
///     .build();
///
/// assert_eq!(provider.name(), "replicate");
/// assert_eq!(provider.color(), "#f59e0b");
/// ```
#[derive(Debug, Clone)]
pub struct ReplicateProvider {
    /// HTTP client for making requests
    pub(crate) client: Client,
    /// Replicate API token
    pub(crate) api_key: String,
    /// API endpoint (e.g., "https://api.replicate.com")
    pub(crate) endpoint: String,
    /// Model alias mappings (e.g., "flux" -> "black-forest-labs/flux-schnell")
    pub(crate) model_mappings: HashMap<String, String>,
    /// Supported generation types
    pub(crate) supported_types: Vec<GenerationType>,
}

impl ReplicateProvider {
    /// Create a new builder for ReplicateProvider
    ///
    /// # Arguments
    ///
    /// * `api_key` - Replicate API token (starts with "r8_")
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::providers::ReplicateProvider;
    ///
    /// let provider = ReplicateProvider::builder("r8_xxx")
    ///     .add_model("flux", "black-forest-labs/flux-schnell")
    ///     .build();
    /// ```
    pub fn builder<S: Into<String>>(api_key: S) -> ReplicateProviderBuilder {
        ReplicateProviderBuilder::new(api_key)
    }

    /// Resolve a model name to its full version string
    ///
    /// If the model name exists in the mappings, returns the mapped version.
    /// Otherwise, returns the model name as-is (allows direct version specification).
    pub(crate) fn resolve_model(&self, request: &GenerationRequest) -> GenerationResult<String> {
        let model = request
            .params
            .model
            .as_ref()
            .ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "Model is required for Replicate provider",
                    Some("model".to_string()),
                )
            })?
            .clone();

        // Check if it's an alias in our mappings
        if let Some(version) = self.model_mappings.get(&model) {
            Ok(version.clone())
        } else {
            // Return as-is (could be a full model version)
            Ok(model)
        }
    }
}

impl GenerationProvider for ReplicateProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if !self.supported_types.contains(&request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "replicate",
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            // Resolve model
            let model = self.resolve_model(&request)?;
            debug!(
                prompt = %request.prompt,
                model = %model,
                "Starting Replicate generation"
            );

            // Build input and create prediction
            let input = build_input(&request);
            let prediction_id =
                create_prediction(&self.client, &self.endpoint, &self.api_key, &model, input)
                    .await?;

            // Poll until complete
            let prediction =
                poll_prediction(&self.client, &self.endpoint, &self.api_key, &prediction_id)
                    .await?;

            // Extract output
            let output_value = prediction.output.ok_or_else(|| {
                GenerationError::provider("No output in prediction response", None, "replicate")
            })?;

            // Handle different output formats
            let output_url = match &output_value {
                // Array of URLs (common for image generation)
                serde_json::Value::Array(arr) if !arr.is_empty() => arr[0]
                    .as_str()
                    .ok_or_else(|| {
                        GenerationError::serialization("Expected URL string in output array")
                    })?
                    .to_string(),
                // Single URL string
                serde_json::Value::String(url) => url.clone(),
                // Object with URL field
                serde_json::Value::Object(obj) => {
                    if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                        url.to_string()
                    } else if let Some(url) = obj.get("audio").and_then(|v| v.as_str()) {
                        url.to_string()
                    } else {
                        return Err(GenerationError::serialization(
                            "Unexpected output format: no URL field found",
                        ));
                    }
                }
                _ => {
                    return Err(GenerationError::serialization(format!(
                        "Unexpected output format: {:?}",
                        output_value
                    )));
                }
            };

            // Fetch the output data
            let (bytes, content_type) = fetch_output(&self.client, &output_url).await?;

            let data = GenerationData::bytes(bytes.clone());

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider("replicate")
                .with_model(model)
                .with_duration(duration)
                .with_size_bytes(bytes.len() as u64);

            if let Some(ct) = content_type {
                metadata = metadata.with_content_type(ct);
            }

            if let Some(ref params) = request.params.width {
                if let Some(ref height) = request.params.height {
                    metadata = metadata.with_dimensions(*params, *height);
                }
            }

            info!(
                duration_ms = duration.as_millis(),
                prediction_id = %prediction_id,
                "Replicate generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional outputs (if array has multiple items)
            if let serde_json::Value::Array(arr) = &output_value {
                if arr.len() > 1 {
                    let mut additional_outputs = Vec::new();
                    for url_value in arr.iter().skip(1) {
                        if let Some(url) = url_value.as_str() {
                            match fetch_output(&self.client, url).await {
                                Ok((bytes, _)) => {
                                    additional_outputs.push(GenerationData::bytes(bytes));
                                }
                                Err(e) => {
                                    warn!(error = %e, url = %url, "Failed to fetch additional output");
                                }
                            }
                        }
                    }
                    if !additional_outputs.is_empty() {
                        output = output.with_additional_outputs(additional_outputs);
                    }
                }
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "replicate"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.supported_types.clone()
    }

    fn supports(&self, gen_type: GenerationType) -> bool {
        self.supported_types.contains(&gen_type)
    }

    fn color(&self) -> &str {
        "#f59e0b" // Replicate amber/orange
    }

    fn default_model(&self) -> Option<&str> {
        // Return the first model in mappings if available
        self.model_mappings.values().next().map(|s| s.as_str())
    }
}

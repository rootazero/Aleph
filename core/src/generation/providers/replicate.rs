//! Replicate API Provider for Media Generation
//!
//! This module implements the `GenerationProvider` trait for Replicate's
//! REST API with polling-based async generation.
//!
//! # API Reference
//!
//! - Create Prediction: POST `{base_url}/v1/predictions`
//! - Get Prediction: GET `{base_url}/v1/predictions/{id}`
//! - Auth: Bearer token
//!
//! # Supported Models
//!
//! - Flux Schnell (fast image generation)
//! - SDXL (high-quality image generation)
//! - MusicGen (audio generation)
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
//! use aethecore::generation::providers::ReplicateProvider;
//!
//! let provider = ReplicateProvider::builder("r8_xxx")
//!     .add_model("flux", "black-forest-labs/flux-schnell")
//!     .add_model("sdxl", "stability-ai/sdxl:39ed52f2...")
//!     .build();
//!
//! let request = GenerationRequest::image("A sunset over mountains")
//!     .with_params(GenerationParams::builder()
//!         .model("flux")
//!         .width(1024)
//!         .height(1024)
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! ```

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// === Constants ===

/// Default API endpoint for Replicate
const DEFAULT_ENDPOINT: &str = "https://api.replicate.com";

/// Default timeout for generation requests (5 minutes)
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Polling interval between status checks (1 second)
const POLL_INTERVAL_MS: u64 = 1000;

/// Maximum number of polling attempts (5 minutes at 1 second intervals)
const MAX_POLL_ATTEMPTS: u32 = 300;

// === Built-in Model Mappings ===

/// Flux Schnell - fast image generation
pub const MODEL_FLUX_SCHNELL: &str = "black-forest-labs/flux-schnell";

/// Stable Diffusion XL - high-quality image generation
pub const MODEL_SDXL: &str =
    "stability-ai/sdxl:39ed52f2a78e934b3ba6e2a89f5b1c712de7dfea535525255b1aa35c5565e08b";

/// Meta MusicGen - audio/music generation
pub const MODEL_MUSICGEN: &str =
    "meta/musicgen:b05b1dff1d8c6dc63d14b0cdb42135378dcb87f6373b0d3d341ede46e59e2b38";

// === Provider Implementation ===

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
/// use aethecore::generation::providers::ReplicateProvider;
/// use aethecore::generation::{GenerationProvider, GenerationType};
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
    client: Client,
    /// Replicate API token
    api_key: String,
    /// API endpoint (e.g., "https://api.replicate.com")
    endpoint: String,
    /// Model alias mappings (e.g., "flux" -> "black-forest-labs/flux-schnell")
    model_mappings: HashMap<String, String>,
    /// Supported generation types
    supported_types: Vec<GenerationType>,
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
    /// use aethecore::generation::providers::ReplicateProvider;
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
    fn resolve_model(&self, request: &GenerationRequest) -> GenerationResult<String> {
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

    /// Build the input object for a prediction request
    fn build_input(&self, request: &GenerationRequest) -> serde_json::Value {
        let mut input = serde_json::Map::new();

        // Always include the prompt
        input.insert("prompt".to_string(), serde_json::json!(request.prompt));

        // Add optional parameters based on generation type
        match request.generation_type {
            GenerationType::Image => {
                if let Some(width) = request.params.width {
                    input.insert("width".to_string(), serde_json::json!(width));
                }
                if let Some(height) = request.params.height {
                    input.insert("height".to_string(), serde_json::json!(height));
                }
                if let Some(n) = request.params.n {
                    input.insert("num_outputs".to_string(), serde_json::json!(n));
                }
                if let Some(seed) = request.params.seed {
                    input.insert("seed".to_string(), serde_json::json!(seed));
                }
                if let Some(ref negative) = request.params.negative_prompt {
                    input.insert("negative_prompt".to_string(), serde_json::json!(negative));
                }
                if let Some(guidance) = request.params.guidance_scale {
                    input.insert("guidance_scale".to_string(), serde_json::json!(guidance));
                }
                if let Some(steps) = request.params.steps {
                    input.insert("num_inference_steps".to_string(), serde_json::json!(steps));
                }
            }
            GenerationType::Audio => {
                if let Some(duration) = request.params.duration_seconds {
                    input.insert("duration".to_string(), serde_json::json!(duration));
                }
                if let Some(ref reference) = request.params.reference_audio {
                    input.insert("melody".to_string(), serde_json::json!(reference));
                }
            }
            GenerationType::Video => {
                if let Some(duration) = request.params.duration_seconds {
                    input.insert("duration".to_string(), serde_json::json!(duration));
                }
                if let Some(fps) = request.params.fps {
                    input.insert("fps".to_string(), serde_json::json!(fps));
                }
            }
            GenerationType::Speech => {
                if let Some(ref voice) = request.params.voice {
                    input.insert("voice".to_string(), serde_json::json!(voice));
                }
                if let Some(speed) = request.params.speed {
                    input.insert("speed".to_string(), serde_json::json!(speed));
                }
            }
        }

        // Add any extra parameters
        for (key, value) in &request.params.extra {
            input.insert(key.clone(), value.clone());
        }

        serde_json::Value::Object(input)
    }

    /// Create a new prediction and return its ID
    async fn create_prediction(
        &self,
        model: &str,
        input: serde_json::Value,
    ) -> GenerationResult<String> {
        let url = format!("{}/v1/predictions", self.endpoint);

        let request_body = CreatePredictionRequest {
            version: model.to_string(),
            input,
        };

        debug!(
            model = %model,
            url = %url,
            "Creating Replicate prediction"
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                } else if e.is_connect() {
                    GenerationError::network(format!("Connection failed: {}", e))
                } else {
                    GenerationError::network(e.to_string())
                }
            })?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            error!(
                status = %status,
                body = %response_text,
                "Replicate prediction creation failed"
            );
            return Err(Self::parse_error_response(status.as_u16(), &response_text));
        }

        let prediction: PredictionResponse = serde_json::from_str(&response_text).map_err(|e| {
            GenerationError::serialization(format!("Failed to parse response: {}", e))
        })?;

        debug!(
            id = %prediction.id,
            status = %prediction.status,
            "Prediction created"
        );

        Ok(prediction.id)
    }

    /// Poll a prediction until it completes or fails
    async fn poll_prediction(&self, id: &str) -> GenerationResult<PredictionResponse> {
        let url = format!("{}/v1/predictions/{}", self.endpoint, id);
        let mut attempts = 0;

        loop {
            attempts += 1;
            if attempts > MAX_POLL_ATTEMPTS {
                return Err(GenerationError::timeout(Duration::from_millis(
                    POLL_INTERVAL_MS * MAX_POLL_ATTEMPTS as u64,
                )));
            }

            let response = self
                .client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await
                .map_err(|e| GenerationError::network(e.to_string()))?;

            let status = response.status();
            let response_text = response
                .text()
                .await
                .map_err(|e| GenerationError::network(format!("Failed to read response: {}", e)))?;

            if !status.is_success() {
                error!(
                    status = %status,
                    body = %response_text,
                    "Failed to poll prediction"
                );
                return Err(Self::parse_error_response(status.as_u16(), &response_text));
            }

            let prediction: PredictionResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            match prediction.status.as_str() {
                "succeeded" => {
                    info!(
                        id = %id,
                        attempts = attempts,
                        "Prediction succeeded"
                    );
                    return Ok(prediction);
                }
                "failed" => {
                    error!(
                        id = %id,
                        error = ?prediction.error,
                        "Prediction failed"
                    );
                    return Err(GenerationError::provider(
                        prediction
                            .error
                            .unwrap_or_else(|| "Prediction failed".to_string()),
                        None,
                        "replicate",
                    ));
                }
                "canceled" => {
                    warn!(id = %id, "Prediction was canceled");
                    return Err(GenerationError::provider(
                        "Prediction was canceled",
                        None,
                        "replicate",
                    ));
                }
                status => {
                    debug!(
                        id = %id,
                        status = %status,
                        attempts = attempts,
                        "Prediction in progress, polling..."
                    );
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
            }
        }
    }

    /// Fetch the output from a URL and return as bytes
    async fn fetch_output(&self, url: &str) -> GenerationResult<(Vec<u8>, Option<String>)> {
        debug!(url = %url, "Fetching prediction output");

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| GenerationError::download(e.to_string(), Some(url.to_string())))?;

        if !response.status().is_success() {
            return Err(GenerationError::download(
                format!("HTTP {}", response.status()),
                Some(url.to_string()),
            ));
        }

        // Get content type from headers
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let bytes = response
            .bytes()
            .await
            .map_err(|e| GenerationError::download(e.to_string(), Some(url.to_string())))?;

        Ok((bytes.to_vec(), content_type))
    }

    /// Parse API error response
    fn parse_error_response(status: u16, body: &str) -> GenerationError {
        // Try to parse as JSON error
        if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(body) {
            let message = error_response
                .detail
                .unwrap_or_else(|| error_response.title.unwrap_or_else(|| body.to_string()));

            // Check for specific error types
            if message.to_lowercase().contains("rate limit") {
                return GenerationError::rate_limit(message, None);
            }
            if message.to_lowercase().contains("unauthorized")
                || message.to_lowercase().contains("invalid token")
            {
                return GenerationError::authentication(message, "replicate");
            }
        }

        // Handle based on status code
        match status {
            401 => GenerationError::authentication("Invalid API token", "replicate"),
            402 => GenerationError::quota_exceeded("Payment required or credits exhausted", None),
            403 => GenerationError::authentication("Access forbidden", "replicate"),
            404 => GenerationError::model_not_found("Model or prediction not found", "replicate"),
            422 => GenerationError::invalid_parameters(body.to_string(), None),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            500..=599 => GenerationError::provider(
                format!("Server error: {}", body),
                Some(status),
                "replicate",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status),
                "replicate",
            ),
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
            let input = self.build_input(&request);
            let prediction_id = self.create_prediction(&model, input).await?;

            // Poll until complete
            let prediction = self.poll_prediction(&prediction_id).await?;

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
            let (bytes, content_type) = self.fetch_output(&output_url).await?;

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
                            match self.fetch_output(url).await {
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

// === Builder ===

/// Builder for ReplicateProvider
///
/// Provides a fluent interface for constructing a ReplicateProvider with
/// custom configuration.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::ReplicateProvider;
/// use aethecore::generation::GenerationType;
///
/// let provider = ReplicateProvider::builder("r8_xxx")
///     .endpoint("https://custom.replicate.com")
///     .add_model("flux", "black-forest-labs/flux-schnell")
///     .add_model("sdxl", "stability-ai/sdxl:39ed52f2...")
///     .supported_types(vec![GenerationType::Image, GenerationType::Audio])
///     .build();
/// ```
#[derive(Debug)]
pub struct ReplicateProviderBuilder {
    api_key: String,
    endpoint: String,
    model_mappings: HashMap<String, String>,
    supported_types: Vec<GenerationType>,
}

impl ReplicateProviderBuilder {
    /// Create a new builder with the given API key
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model_mappings: HashMap::new(),
            supported_types: vec![GenerationType::Image, GenerationType::Audio],
        }
    }

    /// Set a custom API endpoint
    pub fn endpoint<S: Into<String>>(mut self, endpoint: S) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Add a model alias mapping
    ///
    /// # Arguments
    ///
    /// * `alias` - Short name to use in requests (e.g., "flux")
    /// * `model_version` - Full model version string
    pub fn add_model<S: Into<String>>(mut self, alias: S, model_version: S) -> Self {
        self.model_mappings
            .insert(alias.into(), model_version.into());
        self
    }

    /// Set the supported generation types
    pub fn supported_types(mut self, types: Vec<GenerationType>) -> Self {
        self.supported_types = types;
        self
    }

    /// Build the ReplicateProvider
    pub fn build(self) -> ReplicateProvider {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        ReplicateProvider {
            client,
            api_key: self.api_key,
            endpoint: self.endpoint,
            model_mappings: self.model_mappings,
            supported_types: self.supported_types,
        }
    }
}

// === Request/Response Types ===

/// Request body for creating a prediction
#[derive(Debug, Serialize)]
struct CreatePredictionRequest {
    /// Model version to run
    version: String,
    /// Input parameters for the model
    input: serde_json::Value,
}

/// Response from prediction endpoints
#[derive(Debug, Deserialize)]
struct PredictionResponse {
    /// Prediction ID
    id: String,
    /// Current status (starting, processing, succeeded, failed, canceled)
    status: String,
    /// Output data (when succeeded)
    output: Option<serde_json::Value>,
    /// Error message (when failed)
    error: Option<String>,
}

/// Error response format
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    /// Error title
    title: Option<String>,
    /// Error detail message
    detail: Option<String>,
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Builder Tests ===

    #[test]
    fn test_builder_creation_with_defaults() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        assert_eq!(provider.api_key, "r8_test_key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert!(provider.model_mappings.is_empty());
        assert!(provider.supported_types.contains(&GenerationType::Image));
        assert!(provider.supported_types.contains(&GenerationType::Audio));
    }

    #[test]
    fn test_builder_with_custom_endpoint() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .endpoint("https://custom.replicate.com")
            .build();

        assert_eq!(provider.endpoint, "https://custom.replicate.com");
    }

    #[test]
    fn test_builder_with_custom_models() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .add_model("sdxl", MODEL_SDXL)
            .add_model("music", MODEL_MUSICGEN)
            .build();

        assert_eq!(provider.model_mappings.len(), 3);
        assert_eq!(
            provider.model_mappings.get("flux"),
            Some(&MODEL_FLUX_SCHNELL.to_string())
        );
        assert_eq!(
            provider.model_mappings.get("sdxl"),
            Some(&MODEL_SDXL.to_string())
        );
        assert_eq!(
            provider.model_mappings.get("music"),
            Some(&MODEL_MUSICGEN.to_string())
        );
    }

    #[test]
    fn test_builder_with_supported_types() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image])
            .build();

        assert_eq!(provider.supported_types.len(), 1);
        assert!(provider.supported_types.contains(&GenerationType::Image));
        assert!(!provider.supported_types.contains(&GenerationType::Audio));
    }

    // === Model Resolution Tests ===

    #[test]
    fn test_model_resolution_alias() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .build();

        let request = GenerationRequest::image("test")
            .with_params(GenerationParams::builder().model("flux").build());

        let resolved = provider.resolve_model(&request).unwrap();
        assert_eq!(resolved, MODEL_FLUX_SCHNELL);
    }

    #[test]
    fn test_model_resolution_fallback_to_raw() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("test").with_params(
            GenerationParams::builder()
                .model("custom/model:abc123")
                .build(),
        );

        let resolved = provider.resolve_model(&request).unwrap();
        assert_eq!(resolved, "custom/model:abc123");
    }

    #[test]
    fn test_model_resolution_missing_model() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("test");

        let result = provider.resolve_model(&request);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(GenerationError::InvalidParametersError { .. })
        ));
    }

    // === Supports Tests ===

    #[test]
    fn test_supports_types_based_on_config() {
        let image_only = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image])
            .build();

        assert!(image_only.supports(GenerationType::Image));
        assert!(!image_only.supports(GenerationType::Audio));
        assert!(!image_only.supports(GenerationType::Video));
        assert!(!image_only.supports(GenerationType::Speech));

        let audio_video = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Audio, GenerationType::Video])
            .build();

        assert!(!audio_video.supports(GenerationType::Image));
        assert!(audio_video.supports(GenerationType::Audio));
        assert!(audio_video.supports(GenerationType::Video));
    }

    // === Input Building Tests ===

    #[test]
    fn test_input_building_for_image() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("A beautiful sunset").with_params(
            GenerationParams::builder()
                .width(1024)
                .height(768)
                .n(2)
                .seed(42)
                .negative_prompt("blurry")
                .guidance_scale(7.5)
                .steps(50)
                .build(),
        );

        let input = provider.build_input(&request);

        assert_eq!(input["prompt"], "A beautiful sunset");
        assert_eq!(input["width"], 1024);
        assert_eq!(input["height"], 768);
        assert_eq!(input["num_outputs"], 2);
        assert_eq!(input["seed"], 42);
        assert_eq!(input["negative_prompt"], "blurry");
        assert_eq!(input["guidance_scale"], 7.5);
        assert_eq!(input["num_inference_steps"], 50);
    }

    #[test]
    fn test_input_building_for_audio() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::audio("Happy electronic music").with_params(
            GenerationParams::builder()
                .duration_seconds(30.0)
                .reference_audio("https://example.com/melody.mp3")
                .build(),
        );

        let input = provider.build_input(&request);

        assert_eq!(input["prompt"], "Happy electronic music");
        assert_eq!(input["duration"], 30.0);
        assert_eq!(input["melody"], "https://example.com/melody.mp3");
    }

    #[test]
    fn test_input_building_minimal() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("A cat");

        let input = provider.build_input(&request);

        assert_eq!(input["prompt"], "A cat");
        assert!(input.get("width").is_none());
        assert!(input.get("height").is_none());
    }

    // === Prediction Status Parsing Tests ===

    #[test]
    fn test_prediction_response_parsing() {
        let json = r#"{
            "id": "xyz123",
            "status": "succeeded",
            "output": ["https://replicate.delivery/image.png"],
            "error": null
        }"#;

        let prediction: PredictionResponse = serde_json::from_str(json).unwrap();

        assert_eq!(prediction.id, "xyz123");
        assert_eq!(prediction.status, "succeeded");
        assert!(prediction.output.is_some());
        assert!(prediction.error.is_none());
    }

    #[test]
    fn test_prediction_response_failed() {
        let json = r#"{
            "id": "abc456",
            "status": "failed",
            "output": null,
            "error": "Model failed to generate output"
        }"#;

        let prediction: PredictionResponse = serde_json::from_str(json).unwrap();

        assert_eq!(prediction.id, "abc456");
        assert_eq!(prediction.status, "failed");
        assert!(prediction.output.is_none());
        assert_eq!(
            prediction.error,
            Some("Model failed to generate output".to_string())
        );
    }

    // === Output Extraction Tests ===

    #[test]
    fn test_output_extraction_url_array() {
        let output: serde_json::Value = serde_json::json!([
            "https://replicate.delivery/image1.png",
            "https://replicate.delivery/image2.png"
        ]);

        if let serde_json::Value::Array(arr) = &output {
            let url = arr[0].as_str().unwrap();
            assert_eq!(url, "https://replicate.delivery/image1.png");
        }
    }

    #[test]
    fn test_output_extraction_single_url() {
        let output: serde_json::Value = serde_json::json!("https://replicate.delivery/audio.mp3");

        if let serde_json::Value::String(url) = &output {
            assert_eq!(url, "https://replicate.delivery/audio.mp3");
        }
    }

    // === Error Handling Tests ===

    #[test]
    fn test_error_handling_failed_status() {
        let error = ReplicateProvider::parse_error_response(500, "Internal server error");

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    #[test]
    fn test_error_handling_auth() {
        let error = ReplicateProvider::parse_error_response(401, "Invalid token");

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_error_handling_rate_limit() {
        let error = ReplicateProvider::parse_error_response(429, "Rate limit exceeded");

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_error_handling_quota() {
        let error = ReplicateProvider::parse_error_response(402, "Payment required");

        assert!(matches!(error, GenerationError::QuotaExceededError { .. }));
    }

    // === Trait Implementation Tests ===

    #[test]
    fn test_name() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert_eq!(provider.name(), "replicate");
    }

    #[test]
    fn test_color() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert_eq!(provider.color(), "#f59e0b");
    }

    #[test]
    fn test_default_model() {
        let provider_empty = ReplicateProvider::builder("r8_test_key").build();
        assert!(provider_empty.default_model().is_none());

        let provider_with_model = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .build();
        assert!(provider_with_model.default_model().is_some());
    }

    #[test]
    fn test_supported_types_method() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image, GenerationType::Audio])
            .build();

        let types = provider.supported_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&GenerationType::Image));
        assert!(types.contains(&GenerationType::Audio));
    }

    // === Send + Sync Tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ReplicateProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(ReplicateProvider::builder("r8_test").build());

        assert_eq!(provider.name(), "replicate");
        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_builder_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ReplicateProviderBuilder>();
    }

    // === Constants Tests ===

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://api.replicate.com");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 300);
        assert_eq!(POLL_INTERVAL_MS, 1000);
        assert_eq!(MAX_POLL_ATTEMPTS, 300);
    }

    #[test]
    fn test_model_constants() {
        assert!(MODEL_FLUX_SCHNELL.contains("flux-schnell"));
        assert!(MODEL_SDXL.contains("sdxl"));
        assert!(MODEL_MUSICGEN.contains("musicgen"));
    }

    // === Request Serialization Tests ===

    #[test]
    fn test_create_prediction_request_serialization() {
        let request = CreatePredictionRequest {
            version: "test/model:abc123".to_string(),
            input: serde_json::json!({
                "prompt": "A test prompt",
                "width": 1024
            }),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"version\":\"test/model:abc123\""));
        assert!(json.contains("\"prompt\":\"A test prompt\""));
        assert!(json.contains("\"width\":1024"));
    }

    // === Edge Cases ===

    #[test]
    fn test_empty_model_mappings() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert!(provider.model_mappings.is_empty());
    }

    #[test]
    fn test_input_with_extra_params() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("test").with_params(
            GenerationParams::builder()
                .extra("custom_param", serde_json::json!("custom_value"))
                .extra("numeric_param", serde_json::json!(42))
                .build(),
        );

        let input = provider.build_input(&request);

        assert_eq!(input["prompt"], "test");
        assert_eq!(input["custom_param"], "custom_value");
        assert_eq!(input["numeric_param"], 42);
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "title": "Validation Error",
            "detail": "Model version is invalid"
        }"#;

        let error: ErrorResponse = serde_json::from_str(json).unwrap();

        assert_eq!(error.title, Some("Validation Error".to_string()));
        assert_eq!(error.detail, Some("Model version is invalid".to_string()));
    }

    #[test]
    fn test_error_response_minimal() {
        let json = r#"{}"#;

        let error: ErrorResponse = serde_json::from_str(json).unwrap();

        assert!(error.title.is_none());
        assert!(error.detail.is_none());
    }
}

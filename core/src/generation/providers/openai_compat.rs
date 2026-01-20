//! Generic OpenAI-Compatible Image Generation Provider
//!
//! This module implements a configurable `GenerationProvider` for any API that follows
//! OpenAI's image generation format. Use cases include third-party proxies, custom
//! endpoints, and alternative providers.
//!
//! # Key Differences from OpenAiImageProvider
//!
//! - **Configurable name**: Provider name is user-specified, not hardcoded
//! - **Configurable supported_types**: Can support Image, Video, etc.
//! - **Configurable color**: Brand color is user-specified
//! - **Required base_url**: No default endpoint (must be explicitly provided)
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::providers::OpenAiCompatProvider;
//! use aethecore::generation::GenerationType;
//!
//! // Using builder pattern
//! let provider = OpenAiCompatProvider::builder("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
//!     .model("dall-e-3")
//!     .color("#ff0000")
//!     .supported_types(vec![GenerationType::Image])
//!     .build()?;
//!
//! // Using simple constructor
//! let provider = OpenAiCompatProvider::new(
//!     "my-service",
//!     "api-key",
//!     "https://api.myservice.com",
//!     Some("model-name".to_string()),
//! )?;
//! ```

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

/// Default model for image generation
const DEFAULT_MODEL: &str = "dall-e-3";

/// Default timeout for image generation requests (120 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default brand color
const DEFAULT_COLOR: &str = "#6366f1"; // Indigo

/// Generic OpenAI-Compatible Image Generation Provider
///
/// This provider integrates with any API that follows OpenAI's image generation
/// format, making it suitable for third-party proxies, custom endpoints, and
/// alternative providers.
///
/// # Features
///
/// - Configurable provider name and branding
/// - Flexible supported generation types
/// - Compatible with OpenAI image generation API format
/// - Support for both URL and base64 response formats
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::providers::OpenAiCompatProvider;
/// use aethecore::generation::GenerationType;
///
/// let provider = OpenAiCompatProvider::builder("custom-provider", "api-key", "https://api.example.com")
///     .model("dall-e-3")
///     .supported_types(vec![GenerationType::Image])
///     .build()?;
///
/// assert_eq!(provider.name(), "custom-provider");
/// ```
#[derive(Debug, Clone)]
pub struct OpenAiCompatProvider {
    /// Provider name (user-configurable)
    name: String,
    /// HTTP client for making requests
    client: Client,
    /// API key for authentication
    api_key: String,
    /// API endpoint (e.g., "https://api.proxy.com")
    endpoint: String,
    /// Model to use (e.g., "dall-e-3")
    model: String,
    /// Brand color (e.g., "#ff0000")
    color: String,
    /// Supported generation types
    supported_types: Vec<GenerationType>,
}

impl OpenAiCompatProvider {
    /// Create a new OpenAI-compatible provider with simple constructor
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (used for identification and logging)
    /// * `api_key` - API key for authentication
    /// * `base_url` - Base URL for the API endpoint
    /// * `model` - Optional model name (defaults to "dall-e-3")
    ///
    /// # Returns
    ///
    /// Returns a `GenerationResult<Self>` which may fail if the HTTP client
    /// cannot be built or if validation fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::providers::OpenAiCompatProvider;
    ///
    /// let provider = OpenAiCompatProvider::new(
    ///     "my-service",
    ///     "sk-xxx",
    ///     "https://api.myservice.com",
    ///     None,
    /// )?;
    /// ```
    pub fn new<S1, S2, S3>(
        name: S1,
        api_key: S2,
        base_url: S3,
        model: Option<String>,
    ) -> GenerationResult<Self>
    where
        S1: Into<String>,
        S2: Into<String>,
        S3: Into<String>,
    {
        OpenAiCompatProviderBuilder::new(name, api_key, base_url)
            .model(model.unwrap_or_else(|| DEFAULT_MODEL.to_string()))
            .build()
    }

    /// Create a builder for OpenAiCompatProvider
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name
    /// * `api_key` - API key for authentication
    /// * `base_url` - Base URL for the API endpoint
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::providers::OpenAiCompatProvider;
    /// use aethecore::generation::GenerationType;
    ///
    /// let provider = OpenAiCompatProvider::builder("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
    ///     .model("dall-e-3")
    ///     .color("#ff0000")
    ///     .supported_types(vec![GenerationType::Image])
    ///     .timeout_secs(180)
    ///     .build()?;
    /// ```
    pub fn builder<S1, S2, S3>(name: S1, api_key: S2, base_url: S3) -> OpenAiCompatProviderBuilder
    where
        S1: Into<String>,
        S2: Into<String>,
        S3: Into<String>,
    {
        OpenAiCompatProviderBuilder::new(name, api_key, base_url)
    }

    /// Get the full URL for the images/generations endpoint
    fn generations_url(&self) -> String {
        format!("{}/v1/images/generations", self.endpoint)
    }

    /// Get the full URL for the images/edits endpoint
    fn edits_url(&self) -> String {
        format!("{}/v1/images/edits", self.endpoint)
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> ImageGenerationRequest {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        // Build size string from width/height if provided
        let size = match (request.params.width, request.params.height) {
            (Some(w), Some(h)) => Some(format!("{}x{}", w, h)),
            _ => None,
        };

        ImageGenerationRequest {
            model,
            prompt: request.prompt.clone(),
            size,
            quality: request.params.quality.clone(),
            style: request.params.style.clone(),
            n: request.params.n,
            response_format: Some("url".to_string()), // Default to URL format
            user: request.user_id.clone(),
        }
    }

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as OpenAI error format
        if let Ok(error_response) = serde_json::from_str::<OpenAiErrorResponse>(body) {
            let message = error_response.error.message;
            let error_type = error_response.error.error_type;

            // Check for specific error types
            if error_type == "invalid_request_error" {
                // Check for content policy violations
                if message.contains("content policy")
                    || message.contains("safety system")
                    || message.contains("prohibited")
                {
                    return GenerationError::content_filtered(message, None);
                }
                return GenerationError::invalid_parameters(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key or unauthorized", &self.name),
            429 => {
                // Try to extract retry-after from response
                GenerationError::rate_limit("Rate limit exceeded", None)
            }
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                &self.name,
            ),
            404 => GenerationError::model_not_found(&self.model, &self.name),
            500..=599 => GenerationError::provider(
                format!("Server error: {}", body),
                Some(status.as_u16()),
                &self.name,
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                &self.name,
            ),
        }
    }
}

/// Builder for OpenAiCompatProvider
///
/// Provides a fluent interface for constructing an OpenAI-compatible provider
/// with flexible configuration options.
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::providers::OpenAiCompatProviderBuilder;
/// use aethecore::generation::GenerationType;
///
/// let provider = OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com")
///     .model("dall-e-3")
///     .color("#ff0000")
///     .supported_types(vec![GenerationType::Image, GenerationType::Video])
///     .timeout_secs(180)
///     .build()?;
/// ```
#[derive(Debug)]
pub struct OpenAiCompatProviderBuilder {
    /// Provider name
    name: String,
    /// API key for authentication
    api_key: String,
    /// Base URL for the API endpoint
    base_url: String,
    /// Model to use
    model: String,
    /// Brand color
    color: String,
    /// Supported generation types
    supported_types: Vec<GenerationType>,
    /// Request timeout in seconds
    timeout_secs: u64,
}

impl OpenAiCompatProviderBuilder {
    /// Create a new builder with required parameters
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (used for identification and logging)
    /// * `api_key` - API key for authentication
    /// * `base_url` - Base URL for the API endpoint (required, no default)
    pub fn new<S1, S2, S3>(name: S1, api_key: S2, base_url: S3) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
        S3: Into<String>,
    {
        Self {
            name: name.into(),
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: DEFAULT_MODEL.to_string(),
            color: DEFAULT_COLOR.to_string(),
            supported_types: vec![GenerationType::Image],
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    /// Set the model name
    ///
    /// # Arguments
    ///
    /// * `model` - Model identifier (e.g., "dall-e-3", "dall-e-2")
    pub fn model<S: Into<String>>(mut self, model: S) -> Self {
        self.model = model.into();
        self
    }

    /// Set the brand color
    ///
    /// # Arguments
    ///
    /// * `color` - Hex color code (e.g., "#ff0000")
    pub fn color<S: Into<String>>(mut self, color: S) -> Self {
        self.color = color.into();
        self
    }

    /// Set the supported generation types
    ///
    /// # Arguments
    ///
    /// * `types` - List of supported generation types
    pub fn supported_types(mut self, types: Vec<GenerationType>) -> Self {
        self.supported_types = types;
        self
    }

    /// Set the request timeout in seconds
    ///
    /// # Arguments
    ///
    /// * `secs` - Timeout duration in seconds
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Build the OpenAiCompatProvider
    ///
    /// # Returns
    ///
    /// Returns a `GenerationResult<OpenAiCompatProvider>` which may fail if:
    /// - The HTTP client cannot be built
    /// - Required fields are empty
    /// - Validation fails
    ///
    /// # Errors
    ///
    /// - `GenerationError::InvalidParametersError` if name, api_key, or base_url is empty
    /// - `GenerationError::NetworkError` if HTTP client creation fails
    pub fn build(self) -> GenerationResult<OpenAiCompatProvider> {
        // Validate required fields
        if self.name.trim().is_empty() {
            return Err(GenerationError::invalid_parameters(
                "Provider name cannot be empty",
                Some("name".to_string()),
            ));
        }

        if self.api_key.trim().is_empty() {
            return Err(GenerationError::invalid_parameters(
                "API key cannot be empty",
                Some("api_key".to_string()),
            ));
        }

        if self.base_url.trim().is_empty() {
            return Err(GenerationError::invalid_parameters(
                "Base URL cannot be empty",
                Some("base_url".to_string()),
            ));
        }

        if self.supported_types.is_empty() {
            return Err(GenerationError::invalid_parameters(
                "At least one supported type must be specified",
                Some("supported_types".to_string()),
            ));
        }

        // Build HTTP client
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {}", e)))?;

        // Normalize base URL (remove trailing slash and /v1 suffix)
        // This prevents duplicate /v1 in the final URL when user provides "https://api.example.com/v1"
        let endpoint = self
            .base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        Ok(OpenAiCompatProvider {
            name: self.name,
            client,
            api_key: self.api_key,
            endpoint,
            model: self.model,
            color: self.color,
            supported_types: self.supported_types,
        })
    }
}

/// Request body for OpenAI-compatible image generation API
#[derive(Debug, Clone, Serialize)]
struct ImageGenerationRequest {
    /// Model to use (e.g., "dall-e-3")
    model: String,
    /// The prompt to generate an image from
    prompt: String,
    /// Image size (e.g., "1024x1024")
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    /// Quality level ("standard" or "hd")
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<String>,
    /// Style ("vivid" or "natural")
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    /// Number of images to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    /// Response format ("url" or "b64_json")
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
    /// Optional user identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
}

/// Response from OpenAI-compatible image generation API
#[derive(Debug, Clone, Deserialize)]
struct ImageGenerationResponse {
    /// Unix timestamp of when the request was created
    #[allow(dead_code)]
    created: u64,
    /// Array of generated images
    data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Clone, Deserialize)]
struct ImageData {
    /// URL to the generated image (if response_format is "url")
    url: Option<String>,
    /// Base64-encoded image data (if response_format is "b64_json")
    b64_json: Option<String>,
    /// The prompt that was actually used (may differ from input)
    revised_prompt: Option<String>,
}

/// OpenAI API error response format
#[derive(Debug, Clone, Deserialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

/// OpenAI API error details
#[derive(Debug, Clone, Deserialize)]
struct OpenAiError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<String>,
}

impl GenerationProvider for OpenAiCompatProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if !self.supported_types.contains(&request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                provider = %self.name,
                prompt = %request.prompt,
                model = %self.model,
                "Starting OpenAI-compatible image generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.generations_url();

            debug!(url = %url, "Sending request to OpenAI-compatible API");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
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
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    provider = %self.name,
                    status = %status,
                    body = %response_text,
                    "OpenAI-compatible API request failed"
                );
                return Err(self.parse_error_response(status, &response_text));
            }

            // Parse successful response
            let api_response: ImageGenerationResponse = serde_json::from_str(&response_text)
                .map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse OpenAI-compatible response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            // Extract first image
            let first_image = api_response.data.first().ok_or_else(|| {
                GenerationError::provider("No images in response", None, &self.name)
            })?;

            // Convert to GenerationData
            let data = if let Some(url) = &first_image.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &first_image.b64_json {
                // Decode base64 to bytes
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| {
                        GenerationError::serialization(format!("Failed to decode base64: {}", e))
                    })?;
                GenerationData::bytes(bytes)
            } else {
                return Err(GenerationError::provider(
                    "Response contains neither URL nor base64 data",
                    None,
                    &self.name,
                ));
            };

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(&self.name)
                .with_model(body.model.clone())
                .with_duration(duration);

            if let Some(revised) = &first_image.revised_prompt {
                metadata = metadata.with_revised_prompt(revised.clone());
            }

            // Add dimensions from request params
            if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
                metadata = metadata.with_dimensions(w, h);
            }

            info!(
                provider = %self.name,
                duration_ms = duration.as_millis(),
                model = %body.model,
                "OpenAI-compatible image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional images (if n > 1 and provider supports it)
            if api_response.data.len() > 1 {
                let additional: Vec<GenerationData> = api_response
                    .data
                    .iter()
                    .skip(1)
                    .filter_map(|img| {
                        if let Some(url) = &img.url {
                            Some(GenerationData::url(url.clone()))
                        } else if let Some(b64) = &img.b64_json {
                            base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .ok()
                                .map(GenerationData::bytes)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !additional.is_empty() {
                    output = output.with_additional_outputs(additional);
                }
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
        Some(&self.model)
    }

    fn supports_image_editing(&self) -> bool {
        true
    }

    fn edit_image(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }

            // Require reference image
            let reference_image = request.params.reference_image.as_ref().ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "reference_image is required for image editing",
                    Some("reference_image".to_string()),
                )
            })?;

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                provider = %self.name,
                prompt = %request.prompt,
                model = %self.model,
                "Starting OpenAI-compatible image editing"
            );

            // Build multipart form
            let mut form = reqwest::multipart::Form::new();

            // Add model
            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());
            form = form.text("model", model.clone());

            // Add prompt
            form = form.text("prompt", request.prompt.clone());

            // Add image - handle both base64 and URL
            if reference_image.starts_with("http://") || reference_image.starts_with("https://") {
                // Download image from URL first
                let image_bytes = self
                    .client
                    .get(reference_image)
                    .send()
                    .await
                    .map_err(|e| {
                        GenerationError::network(format!("Failed to download image: {}", e))
                    })?
                    .bytes()
                    .await
                    .map_err(|e| {
                        GenerationError::network(format!("Failed to read image bytes: {}", e))
                    })?;

                let part = reqwest::multipart::Part::bytes(image_bytes.to_vec())
                    .file_name("image.png")
                    .mime_str("image/png")
                    .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
                form = form.part("image", part);
            } else {
                // Assume base64-encoded data
                let image_bytes = base64::engine::general_purpose::STANDARD
                    .decode(reference_image)
                    .map_err(|e| {
                        GenerationError::invalid_parameters(
                            format!("Invalid base64 image data: {}", e),
                            Some("reference_image".to_string()),
                        )
                    })?;

                let part = reqwest::multipart::Part::bytes(image_bytes)
                    .file_name("image.png")
                    .mime_str("image/png")
                    .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
                form = form.part("image", part);
            }

            // Add optional mask if provided via extra params
            if let Some(mask_value) = request.params.extra.get("mask") {
                if let Some(mask_str) = mask_value.as_str() {
                    let mask_bytes = base64::engine::general_purpose::STANDARD
                        .decode(mask_str)
                        .map_err(|e| {
                            GenerationError::invalid_parameters(
                                format!("Invalid base64 mask data: {}", e),
                                Some("mask".to_string()),
                            )
                        })?;

                    let part = reqwest::multipart::Part::bytes(mask_bytes)
                        .file_name("mask.png")
                        .mime_str("image/png")
                        .map_err(|e| GenerationError::invalid_parameters(e.to_string(), None))?;
                    form = form.part("mask", part);
                }
            }

            // Add optional size
            if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
                form = form.text("size", format!("{}x{}", w, h));
            }

            // Add optional n (number of images)
            if let Some(n) = request.params.n {
                form = form.text("n", n.to_string());
            }

            // Add response format
            form = form.text("response_format", "url");

            // Add optional user
            if let Some(user) = &request.user_id {
                form = form.text("user", user.clone());
            }

            let url = self.edits_url();
            debug!(url = %url, "Sending edit request to OpenAI-compatible API");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .multipart(form)
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
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    provider = %self.name,
                    status = %status,
                    body = %response_text,
                    "OpenAI-compatible image edit request failed"
                );
                return Err(self.parse_error_response(status, &response_text));
            }

            // Parse successful response (same format as generations)
            let api_response: ImageGenerationResponse = serde_json::from_str(&response_text)
                .map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse OpenAI-compatible edit response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            // Extract first image
            let first_image = api_response.data.first().ok_or_else(|| {
                GenerationError::provider("No images in response", None, &self.name)
            })?;

            // Convert to GenerationData
            let data = if let Some(url) = &first_image.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &first_image.b64_json {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| {
                        GenerationError::serialization(format!("Failed to decode base64: {}", e))
                    })?;
                GenerationData::bytes(bytes)
            } else {
                return Err(GenerationError::provider(
                    "Response contains neither URL nor base64 data",
                    None,
                    &self.name,
                ));
            };

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(&self.name)
                .with_model(model)
                .with_duration(duration);

            if let Some(revised) = &first_image.revised_prompt {
                metadata = metadata.with_revised_prompt(revised.clone());
            }

            if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
                metadata = metadata.with_dimensions(w, h);
            }

            info!(
                provider = %self.name,
                duration_ms = duration.as_millis(),
                "OpenAI-compatible image editing completed"
            );

            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional images
            if api_response.data.len() > 1 {
                let additional: Vec<GenerationData> = api_response
                    .data
                    .iter()
                    .skip(1)
                    .filter_map(|img| {
                        if let Some(url) = &img.url {
                            Some(GenerationData::url(url.clone()))
                        } else if let Some(b64) = &img.b64_json {
                            base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .ok()
                                .map(GenerationData::bytes)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !additional.is_empty() {
                    output = output.with_additional_outputs(additional);
                }
            }

            Ok(output)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Builder tests ===

    #[test]
    fn test_builder_new() {
        let builder =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com");

        assert_eq!(builder.name, "my-proxy");
        assert_eq!(builder.api_key, "sk-xxx");
        assert_eq!(builder.base_url, "https://api.proxy.com");
        assert_eq!(builder.model, DEFAULT_MODEL);
        assert_eq!(builder.color, DEFAULT_COLOR);
        assert_eq!(builder.timeout_secs, DEFAULT_TIMEOUT_SECS);
    }

    #[test]
    fn test_builder_with_model() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .model("dall-e-2");

        assert_eq!(builder.model, "dall-e-2");
    }

    #[test]
    fn test_builder_with_color() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .color("#ff0000");

        assert_eq!(builder.color, "#ff0000");
    }

    #[test]
    fn test_builder_with_supported_types() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image, GenerationType::Video]);

        assert_eq!(builder.supported_types.len(), 2);
        assert!(builder.supported_types.contains(&GenerationType::Image));
        assert!(builder.supported_types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_builder_with_timeout() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .timeout_secs(180);

        assert_eq!(builder.timeout_secs, 180);
    }

    #[test]
    fn test_builder_chaining() {
        let builder =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com")
                .model("custom-model")
                .color("#00ff00")
                .supported_types(vec![GenerationType::Image])
                .timeout_secs(60);

        assert_eq!(builder.name, "my-proxy");
        assert_eq!(builder.model, "custom-model");
        assert_eq!(builder.color, "#00ff00");
        assert_eq!(builder.timeout_secs, 60);
    }

    #[test]
    fn test_builder_build_success() {
        let provider =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
                .model("dall-e-3")
                .color("#ff0000")
                .build()
                .unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff0000");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    #[test]
    fn test_builder_build_normalizes_url() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com/")
            .build()
            .unwrap();

        assert_eq!(provider.endpoint, "https://api.example.com");
    }

    // === Validation tests ===

    #[test]
    fn test_builder_empty_name_fails() {
        let result = OpenAiCompatProviderBuilder::new("", "key", "https://api.example.com").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("name".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_whitespace_name_fails() {
        let result =
            OpenAiCompatProviderBuilder::new("   ", "key", "https://api.example.com").build();

        assert!(result.is_err());
    }

    #[test]
    fn test_builder_empty_api_key_fails() {
        let result =
            OpenAiCompatProviderBuilder::new("proxy", "", "https://api.example.com").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("api_key".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_empty_base_url_fails() {
        let result = OpenAiCompatProviderBuilder::new("proxy", "key", "").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("base_url".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_empty_supported_types_fails() {
        let result = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![])
            .build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("supported_types".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    // === Simple constructor tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider =
            OpenAiCompatProvider::new("my-service", "sk-xxx", "https://api.example.com", None)
                .unwrap();

        assert_eq!(provider.name(), "my-service");
        assert_eq!(provider.default_model(), Some(DEFAULT_MODEL));
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = OpenAiCompatProvider::new(
            "my-service",
            "sk-xxx",
            "https://api.example.com",
            Some("custom-model".to_string()),
        )
        .unwrap();

        assert_eq!(provider.default_model(), Some("custom-model"));
    }

    #[test]
    fn test_new_validation_fails() {
        let result = OpenAiCompatProvider::new("", "key", "https://api.example.com", None);
        assert!(result.is_err());
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider =
            OpenAiCompatProvider::new("custom-name", "key", "https://api.example.com", None)
                .unwrap();

        assert_eq!(provider.name(), "custom-name");
    }

    #[test]
    fn test_supported_types_default() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supported_types_custom() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image, GenerationType::Video])
            .build()
            .unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&GenerationType::Image));
        assert!(types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_supports() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image])
            .build()
            .unwrap();

        assert!(provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_color_default() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(provider.color(), DEFAULT_COLOR);
    }

    #[test]
    fn test_color_custom() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .color("#ff5500")
            .build()
            .unwrap();

        assert_eq!(provider.color(), "#ff5500");
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .model("custom-model-v2")
            .build()
            .unwrap();

        assert_eq!(provider.default_model(), Some("custom-model-v2"));
    }

    // === URL generation tests ===

    #[test]
    fn test_generations_url() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_trailing_slash() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_v1_suffix() {
        // User provides URL with /v1 suffix (common pattern for OpenAI-compatible APIs)
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://ai.t8star.cn/v1", None).unwrap();

        // Should NOT produce duplicate /v1
        assert_eq!(
            provider.generations_url(),
            "https://ai.t8star.cn/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_v1_and_trailing_slash() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/v1/", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "dall-e-3");
        assert_eq!(body.prompt, "A beautiful sunset");
        assert!(body.size.is_none());
        assert!(body.quality.is_none());
        assert!(body.style.is_none());
        assert!(body.n.is_none());
        assert_eq!(body.response_format, Some("url".to_string()));
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset")
            .with_params(
                GenerationParams::builder()
                    .width(1024)
                    .height(1024)
                    .quality("hd")
                    .style("vivid")
                    .n(1)
                    .build(),
            )
            .with_user_id("user-123");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "dall-e-3");
        assert_eq!(body.prompt, "A beautiful sunset");
        assert_eq!(body.size, Some("1024x1024".to_string()));
        assert_eq!(body.quality, Some("hd".to_string()));
        assert_eq!(body.style, Some("vivid".to_string()));
        assert_eq!(body.n, Some(1));
        assert_eq!(body.user, Some("user-123".to_string()));
    }

    #[test]
    fn test_build_request_body_with_custom_model() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A test prompt")
            .with_params(GenerationParams::builder().model("custom-model").build());

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "custom-model");
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let provider =
            OpenAiCompatProvider::new("my-proxy", "key", "https://api.example.com", None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::UNAUTHORIZED, "Unauthorized");

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::TOO_MANY_REQUESTS, "Rate limited");

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_content_policy() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let body = r#"{
            "error": {
                "message": "Request rejected due to content policy violation",
                "type": "invalid_request_error",
                "param": null,
                "code": "content_policy_violation"
            }
        }"#;

        let error = provider.parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::ContentFilteredError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_invalid_params() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let body = r#"{
            "error": {
                "message": "Invalid size parameter",
                "type": "invalid_request_error",
                "param": "size",
                "code": null
            }
        }"#;

        let error = provider.parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let provider =
            OpenAiCompatProvider::new("my-proxy", "key", "https://api.example.com", None).unwrap();
        let error = provider
            .parse_error_response(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "Server error");

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Response parsing tests ===

    #[test]
    fn test_parse_api_response_url() {
        let json = r#"{
            "created": 1700000000,
            "data": [{
                "url": "https://example.com/image.png",
                "revised_prompt": "A beautiful sunset"
            }]
        }"#;

        let response: ImageGenerationResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.created, 1700000000);
        assert_eq!(response.data.len(), 1);
        assert!(response.data[0].url.is_some());
        assert!(response.data[0].revised_prompt.is_some());
    }

    #[test]
    fn test_parse_api_response_b64() {
        let json = r#"{
            "created": 1700000000,
            "data": [{
                "b64_json": "iVBORw0KGgo="
            }]
        }"#;

        let response: ImageGenerationResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.data.len(), 1);
        assert!(response.data[0].b64_json.is_some());
        assert!(response.data[0].url.is_none());
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization_minimal() {
        let request = ImageGenerationRequest {
            model: "dall-e-3".to_string(),
            prompt: "A test prompt".to_string(),
            size: None,
            quality: None,
            style: None,
            n: None,
            response_format: Some("url".to_string()),
            user: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"dall-e-3\""));
        assert!(json.contains("\"prompt\":\"A test prompt\""));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"size\""));
        assert!(!json.contains("\"quality\""));
        assert!(!json.contains("\"style\""));
        assert!(!json.contains("\"n\""));
        assert!(!json.contains("\"user\""));
    }

    #[test]
    fn test_request_serialization_full() {
        let request = ImageGenerationRequest {
            model: "dall-e-3".to_string(),
            prompt: "A test prompt".to_string(),
            size: Some("1024x1024".to_string()),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            n: Some(1),
            response_format: Some("url".to_string()),
            user: Some("user-123".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"size\":\"1024x1024\""));
        assert!(json.contains("\"quality\":\"hd\""));
        assert!(json.contains("\"style\":\"vivid\""));
        assert!(json.contains("\"n\":1"));
        assert!(json.contains("\"user\":\"user-123\""));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAiCompatProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> = Arc::new(
            OpenAiCompatProvider::new("test-proxy", "sk-test", "https://api.example.com", None)
                .unwrap(),
        );

        assert_eq!(provider.name(), "test-proxy");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Builder static method test ===

    #[test]
    fn test_static_builder_method() {
        let provider =
            OpenAiCompatProvider::builder("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
                .model("dall-e-3")
                .color("#ff0000")
                .supported_types(vec![GenerationType::Image])
                .build()
                .unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff0000");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    // === Image editing tests ===

    #[test]
    fn test_supports_image_editing() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert!(provider.supports_image_editing());
    }

    #[test]
    fn test_edits_url() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(
            provider.edits_url(),
            "https://api.example.com/v1/images/edits"
        );
    }

    #[test]
    fn test_edits_url_with_v1_suffix() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/v1", None).unwrap();

        // Should NOT produce duplicate /v1
        assert_eq!(
            provider.edits_url(),
            "https://api.example.com/v1/images/edits"
        );
    }

    #[tokio::test]
    async fn test_edit_image_requires_reference_image() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        // Request without reference_image should fail
        let request = GenerationRequest::image("Add a hat");
        let result = provider.edit_image(request).await;

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("reference_image".to_string()));
        } else {
            panic!("Expected InvalidParametersError, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_edit_image_wrong_type_fails() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        // Video request should fail
        let request = GenerationRequest::video("Edit this video").with_params(
            GenerationParams::builder()
                .reference_image("base64data")
                .build(),
        );

        let result = provider.edit_image(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(GenerationError::UnsupportedGenerationTypeError { .. })
        ));
    }
}

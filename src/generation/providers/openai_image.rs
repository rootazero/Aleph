//! `OpenAI` DALL-E 3 Image Generation Provider
//!
//! This module implements the `GenerationProvider` trait for `OpenAI`'s DALL-E 3 image generation API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/images/generations`
//! - Auth: Bearer token
//! - Request body: `{ model, prompt, size?, quality?, style?, n?, response_format? }`
//! - Response: `{ data: [{ url?, b64_json?, revised_prompt? }], created }`
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::{GenerationProvider, GenerationRequest};
//! use alephcore::generation::providers::OpenAiImageProvider;
//!
//! let provider = OpenAiImageProvider::new("sk-...", None, None, None);
//!
//! let request = GenerationRequest::image("A sunset over mountains")
//!     .with_params(GenerationParams::builder()
//!         .width(1024)
//!         .height(1024)
//!         .quality("hd")
//!         .style("vivid")
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! ```

use super::url_normalize::ResolvedUrl;
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

/// Default API endpoint for `OpenAI`
const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Default model for image generation
const DEFAULT_MODEL: &str = "dall-e-3";

/// Default timeout for image generation requests (120 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// `OpenAI` Image Generation Provider (DALL-E 3)
///
/// This provider integrates with `OpenAI`'s image generation API to create images
/// from text prompts using DALL-E 3.
///
/// # Features
///
/// - DALL-E 3 image generation
/// - Configurable size, quality, and style
/// - Support for both URL and base64 response formats
/// - Automatic prompt revision tracking
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::generation::providers::OpenAiImageProvider;
///
/// let provider = OpenAiImageProvider::new(
///     "sk-your-api-key",
///     None, // Use default endpoint
///     None, // Use default model (dall-e-3)
///     None, // Use default URL resolution
/// );
///
/// assert_eq!(provider.name(), "openai-image");
/// ```
#[derive(Clone)]
pub struct OpenAiImageProvider {
    /// HTTP client for making requests
    client: Client,
    /// `OpenAI` API key (redacted in Debug)
    api_key: String,
    /// API endpoint — fully resolved generations URL
    endpoint: String,
    /// Model to use (e.g., "dall-e-3")
    model: String,
    /// Resolved URL for deriving secondary endpoints (e.g., image edits)
    resolved: ResolvedUrl,
}

impl std::fmt::Debug for OpenAiImageProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiImageProvider")
            .field("client", &self.client)
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("resolved", &self.resolved)
            .finish()
    }
}

impl OpenAiImageProvider {
    /// Create a new `OpenAI` Image Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - `OpenAI` API key
    /// * `base_url` - Optional custom API endpoint (defaults to "<https://api.openai.com>")
    /// * `model` - Optional model name (defaults to "dall-e-3")
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::providers::OpenAiImageProvider;
    ///
    /// // Default configuration
    /// let provider = OpenAiImageProvider::new("sk-xxx", None, None, None);
    ///
    /// // Custom endpoint (e.g., for Azure OpenAI)
    /// let azure_provider = OpenAiImageProvider::new(
    ///     "api-key",
    ///     Some("https://my-resource.openai.azure.com".to_string()),
    ///     Some("dall-e-3".to_string()),
    ///     None,
    /// );
    /// ```
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        resolved_url: Option<ResolvedUrl>,
    ) -> GenerationResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        // Use pre-resolved URL if available, otherwise resolve from base_url
        let resolved = resolved_url.unwrap_or_else(|| {
            let url = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            super::url_normalize::resolve_base_url(&url)
        });
        let endpoint = resolved.primary_endpoint(GenerationType::Image);

        Ok(Self {
            client,
            api_key: api_key.into(),
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            resolved,
        })
    }

    /// Get the full URL for the images/generations endpoint
    fn generations_url(&self) -> String {
        self.endpoint.clone()
    }

    /// Get the image edits endpoint URL.
    /// Returns None for custom URLs or when secondary endpoint is unavailable.
    #[must_use]
    pub fn edits_url(&self) -> Option<String> {
        self.resolved.secondary_endpoint(GenerationType::Image)
    }

    /// Build the API request body from a `GenerationRequest`
    fn build_request_body(
        &self,
        request: &GenerationRequest,
    ) -> GenerationResult<ImageGenerationRequest> {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        // Build size string from width/height if both are provided and positive,
        // then validate against the provider's allow-list BEFORE the HTTP call.
        // The previous code forwarded any `w x h` string to DALL-E and let the
        // provider 400 back `invalid_parameters` — a wasted round-trip whose
        // error message obscured what sizes ARE accepted.
        let size = match (request.params.width, request.params.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                let candidate = format!("{w}x{h}");
                const ALLOWED_SIZES: &[&str] = &["1024x1024", "1024x1792", "1792x1024", "512x512"];
                if !ALLOWED_SIZES.contains(&candidate.as_str()) {
                    return Err(GenerationError::unsupported_dimension(
                        format!(
                            "size '{candidate}' is not supported by DALL-E; use one of {}",
                            ALLOWED_SIZES.join(", ")
                        ),
                        Some("1024x1024".to_string()),
                    ));
                }
                Some(candidate)
            }
            _ => None,
        };

        Ok(ImageGenerationRequest {
            model,
            prompt: request.prompt.clone(),
            size,
            quality: request.params.quality.clone(),
            style: request.params.style.clone(),
            n: request.params.n,
            response_format: Some("url".to_string()), // Default to URL format
            user: request.user_id.clone(),
        })
    }

    /// Parse API error response and convert to `GenerationError`
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
            401 => {
                GenerationError::authentication("Invalid API key or unauthorized", "openai-image")
            }
            429 => {
                // Note: retry-after could be extracted from response headers if available
                GenerationError::rate_limit("Rate limit exceeded", None)
            }
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                "openai-image",
            ),
            404 => GenerationError::model_not_found(&self.model, "openai-image"),
            500..=599 => GenerationError::provider(
                format!("OpenAI server error: {body}"),
                Some(status.as_u16()),
                "openai-image",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {body}"),
                Some(status.as_u16()),
                "openai-image",
            ),
        }
    }
}

/// Request body for `OpenAI` image generation API
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
    /// Number of images to generate (only 1 supported for DALL-E 3)
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    /// Response format ("url" or "`b64_json`")
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
    /// Optional user identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
}

/// Response from `OpenAI` image generation API
#[derive(Debug, Clone, Deserialize)]
struct ImageGenerationResponse {
    /// Unix timestamp of when the request was created
    #[allow(dead_code)] // Deserialized from API response
    created: u64,
    /// Array of generated images
    data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Clone, Deserialize)]
struct ImageData {
    /// URL to the generated image (if `response_format` is "url")
    url: Option<String>,
    /// Base64-encoded image data (if `response_format` is "`b64_json`")
    b64_json: Option<String>,
    /// The prompt that was actually used (may differ from input)
    revised_prompt: Option<String>,
}

/// `OpenAI` API error response format
#[derive(Debug, Clone, Deserialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

/// `OpenAI` API error details
#[derive(Debug, Clone, Deserialize)]
struct OpenAiError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API response
    code: Option<String>,
}

impl GenerationProvider for OpenAiImageProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "openai-image",
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                prompt_len = request.prompt.len(),
                model = %self.model,
                "Starting OpenAI image generation"
            );

            // Build request body
            let body = self.build_request_body(&request)?;
            let url = self.generations_url();

            debug!(url = %url, "Sending request to OpenAI");

            // Single attempt: HTTP call + response classification, retried by
            // `retry_transient` on 429/5xx/timeout/network errors (the
            // classifications in `parse_error_response` mark those
            // `is_retryable()`). Non-retryable errors (auth, content filter,
            // invalid params, serialization) return on the first attempt.
            let attempt = || async {
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
                            GenerationError::network(format!("Connection failed: {e}"))
                        } else {
                            GenerationError::network(e.to_string())
                        }
                    })?;

                let status = response.status();
                let response_text = response.text().await.map_err(|e| {
                    GenerationError::network(format!("Failed to read response body: {e}"))
                })?;

                // Handle non-success status codes
                if !status.is_success() {
                    error!(
                        status = %status,
                        body = %response_text,
                        "OpenAI API request failed"
                    );
                    return Err(self.parse_error_response(status, &response_text));
                }

                // Parse successful response
                serde_json::from_str::<ImageGenerationResponse>(&response_text).map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse OpenAI response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {e}"))
                })
            };

            let api_response =
                super::http::retry_transient("openai-image.generate", attempt).await?;

            // Extract first image (DALL-E 3 only supports n=1)
            let first_image = api_response.data.first().ok_or_else(|| {
                GenerationError::provider("No images in response", None, "openai-image")
            })?;

            // Convert to GenerationData
            let data = if let Some(url) = &first_image.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &first_image.b64_json {
                // Decode base64 to bytes
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| {
                        GenerationError::serialization(format!("Failed to decode base64: {e}"))
                    })?;
                GenerationData::bytes(bytes)
            } else {
                return Err(GenerationError::provider(
                    "Response contains neither URL nor base64 data",
                    None,
                    "openai-image",
                ));
            };

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider("openai-image")
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
                duration_ms = duration.as_millis(),
                model = %body.model,
                "OpenAI image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Image, data).with_metadata(metadata);

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
                            match base64::engine::general_purpose::STANDARD.decode(b64) {
                                Ok(bytes) => Some(GenerationData::bytes(bytes)),
                                Err(e) => {
                                    error!(error = %e, "Failed to decode base64 for additional image");
                                    None
                                }
                            }
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
        "openai-image"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        "#10a37f" // OpenAI green
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();

        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(
            provider.endpoint,
            format!("{}/v1/images/generations", DEFAULT_ENDPOINT)
        );
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = OpenAiImageProvider::new(
            "sk-test-key",
            Some("https://custom.openai.com".to_string()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            provider.endpoint,
            "https://custom.openai.com/v1/images/generations"
        );
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider =
            OpenAiImageProvider::new("sk-test-key", None, Some("dall-e-2".to_string()), None)
                .unwrap();

        assert_eq!(provider.model, "dall-e-2");
    }

    #[test]
    fn test_generations_url() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(
            provider.generations_url(),
            "https://api.openai.com/v1/images/generations"
        );

        let custom_provider = OpenAiImageProvider::new(
            "sk-test-key",
            Some("https://api.example.com".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            custom_provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_url_normalization_with_v1_suffix() {
        // User provides URL with /v1 suffix - should NOT produce duplicate /v1
        let provider = OpenAiImageProvider::new(
            "sk-test-key",
            Some("https://ai.t8star.cn/v1".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            provider.endpoint,
            "https://ai.t8star.cn/v1/images/generations"
        );
        assert_eq!(
            provider.generations_url(),
            "https://ai.t8star.cn/v1/images/generations"
        );
    }

    #[test]
    fn test_url_normalization_with_trailing_slash() {
        let provider = OpenAiImageProvider::new(
            "sk-test-key",
            Some("https://api.example.com/".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            provider.endpoint,
            "https://api.example.com/v1/images/generations"
        );
        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_url_normalization_with_v1_and_trailing_slash() {
        let provider = OpenAiImageProvider::new(
            "sk-test-key",
            Some("https://api.example.com/v1/".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            provider.endpoint,
            "https://api.example.com/v1/images/generations"
        );
        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.name(), "openai-image");
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supports() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_color() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.default_model(), Some("dall-e-3"));

        let custom_provider =
            OpenAiImageProvider::new("sk-test-key", None, Some("dall-e-2".to_string()), None)
                .unwrap();
        assert_eq!(custom_provider.default_model(), Some("dall-e-2"));
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request).unwrap();

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
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
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

        let body = provider.build_request_body(&request).unwrap();

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
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let request = GenerationRequest::image("A test prompt")
            .with_params(GenerationParams::builder().model("dall-e-2").build());

        let body = provider.build_request_body(&request).unwrap();

        assert_eq!(body.model, "dall-e-2");
    }

    /// Regression: an unsupported DALL-E size must be rejected locally with
    /// `unsupported_dimension` instead of burning an HTTP round-trip for a
    /// provider-side 400.
    #[test]
    fn test_build_request_body_rejects_unsupported_size() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let request = GenerationRequest::image("A test prompt")
            .with_params(GenerationParams::builder().width(512).height(768).build());

        let err = provider.build_request_body(&request).unwrap_err();
        assert!(
            err.to_string().contains("not supported"),
            "error should name the unsupported size: {err}"
        );
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::UNAUTHORIZED, "Unauthorized");

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let error = provider.parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_content_policy() {
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let body = r#"{
            "error": {
                "message": "Your request was rejected as a result of our safety system. The prompt may contain content policy violations.",
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
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
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
        let provider = OpenAiImageProvider::new("sk-test-key", None, None, None).unwrap();
        let error = provider.parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        );

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
                "url": "https://oaidalleapiprodscus.blob.core.windows.net/private/image.png",
                "revised_prompt": "A stunning sunset over mountain peaks with golden hour lighting"
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
                "b64_json": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
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
        assert_send_sync::<OpenAiImageProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(OpenAiImageProvider::new("sk-test", None, None, None).unwrap());

        assert_eq!(provider.name(), "openai-image");
        assert!(provider.supports(GenerationType::Image));
    }
}

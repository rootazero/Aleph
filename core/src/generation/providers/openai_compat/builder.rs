//! Builder pattern for OpenAiCompatProvider
//!
//! Provides a fluent interface for constructing an OpenAI-compatible provider
//! with flexible configuration options.

use crate::generation::{GenerationError, GenerationResult, GenerationType};
use reqwest::Client;
use std::time::Duration;

use super::provider::OpenAiCompatProvider;
use super::types::{DEFAULT_COLOR, DEFAULT_MODEL, DEFAULT_TIMEOUT_SECS};

/// Builder for OpenAiCompatProvider
///
/// Provides a fluent interface for constructing an OpenAI-compatible provider
/// with flexible configuration options.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::generation::providers::OpenAiCompatProviderBuilder;
/// use alephcore::generation::GenerationType;
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
    pub(crate) name: String,
    /// API key for authentication
    pub(crate) api_key: String,
    /// Base URL for the API endpoint
    pub(crate) base_url: String,
    /// Model to use
    pub(crate) model: String,
    /// Brand color
    pub(crate) color: String,
    /// Supported generation types
    pub(crate) supported_types: Vec<GenerationType>,
    /// Request timeout in seconds
    pub(crate) timeout_secs: u64,
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

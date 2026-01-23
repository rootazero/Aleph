//! Core provider struct for OpenAI-compatible API
//!
//! Contains the main `OpenAiCompatProvider` struct definition.

use crate::generation::GenerationType;
use reqwest::Client;

use super::builder::OpenAiCompatProviderBuilder;
use super::types::DEFAULT_MODEL;
use crate::generation::GenerationResult;

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
    pub(crate) name: String,
    /// HTTP client for making requests
    pub(crate) client: Client,
    /// API key for authentication
    pub(crate) api_key: String,
    /// API endpoint (e.g., "https://api.proxy.com")
    pub(crate) endpoint: String,
    /// Model to use (e.g., "dall-e-3")
    pub(crate) model: String,
    /// Brand color (e.g., "#ff0000")
    pub(crate) color: String,
    /// Supported generation types
    pub(crate) supported_types: Vec<GenerationType>,
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
}

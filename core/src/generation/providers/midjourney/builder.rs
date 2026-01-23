//! Builder for MidjourneyProvider
//!
//! Provides a fluent interface for constructing a Midjourney provider
//! with flexible configuration options.

use reqwest::Client;
use std::time::Duration;

use super::provider::MidjourneyProvider;
use super::types::{
    MidjourneyMode, DEFAULT_COLOR, DEFAULT_ENDPOINT, DEFAULT_REQUEST_TIMEOUT_SECS, PROVIDER_NAME,
};

/// Builder for MidjourneyProvider
///
/// Provides a fluent interface for constructing a Midjourney provider
/// with flexible configuration options.
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::providers::{MidjourneyProviderBuilder, MidjourneyMode};
///
/// let provider = MidjourneyProviderBuilder::new("your-api-key")
///     .mode(MidjourneyMode::Fast)
///     .color("#5865F2")
///     .timeout_secs(60)
///     .build();
/// ```
#[derive(Debug)]
pub struct MidjourneyProviderBuilder {
    /// API key for authentication
    pub(crate) api_key: String,
    /// API endpoint
    pub(crate) endpoint: String,
    /// Generation mode
    pub(crate) mode: MidjourneyMode,
    /// Brand color
    pub(crate) color: String,
    /// Request timeout in seconds
    pub(crate) timeout_secs: u64,
}

impl MidjourneyProviderBuilder {
    /// Create a new builder with required API key
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            mode: MidjourneyMode::default(),
            color: DEFAULT_COLOR.to_string(),
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Set the generation mode
    ///
    /// # Arguments
    ///
    /// * `mode` - Fast or Relax mode
    pub fn mode(mut self, mode: MidjourneyMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the API endpoint
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Custom API endpoint URL
    pub fn endpoint<S: Into<String>>(mut self, endpoint: S) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Set the brand color
    ///
    /// # Arguments
    ///
    /// * `color` - Hex color code (e.g., "#5865F2")
    pub fn color<S: Into<String>>(mut self, color: S) -> Self {
        self.color = color.into();
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

    /// Build the MidjourneyProvider
    pub fn build(self) -> MidjourneyProvider {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        // Normalize endpoint (remove trailing slash)
        let endpoint = self.endpoint.trim_end_matches('/').to_string();

        MidjourneyProvider {
            name: PROVIDER_NAME.to_string(),
            client,
            api_key: self.api_key,
            endpoint,
            mode: self.mode,
            color: self.color,
        }
    }
}

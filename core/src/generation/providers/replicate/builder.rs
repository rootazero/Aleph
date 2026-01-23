//! Builder for ReplicateProvider
//!
//! Provides a fluent interface for constructing a ReplicateProvider with
//! custom configuration.

use super::constants::DEFAULT_ENDPOINT;
use super::provider::ReplicateProvider;
use crate::generation::GenerationType;
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;

use super::constants::DEFAULT_TIMEOUT_SECS;

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
    pub(crate) api_key: String,
    pub(crate) endpoint: String,
    pub(crate) model_mappings: HashMap<String, String>,
    pub(crate) supported_types: Vec<GenerationType>,
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

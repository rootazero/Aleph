//! Builder for `ReplicateProvider`
//!
//! Provides a fluent interface for constructing a `ReplicateProvider` with
//! custom configuration.

use super::constants::DEFAULT_ENDPOINT;
use super::provider::ReplicateProvider;
use crate::generation::{GenerationError, GenerationType};
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;

use super::constants::DEFAULT_TIMEOUT_SECS;

/// Builder for `ReplicateProvider`
///
/// Provides a fluent interface for constructing a `ReplicateProvider` with
/// custom configuration.
///
/// # Example
///
/// Crate-internal: reached through `create_provider`, never from outside
/// the crate, so this example is illustrative and not compiled.
/// ```rust,ignore
/// use alephcore::generation::providers::ReplicateProvider;
/// use alephcore::generation::GenerationType;
///
/// let provider = ReplicateProvider::builder("r8_xxx")
///     .endpoint("https://custom.replicate.com")
///     .add_model("flux", "black-forest-labs/flux-schnell")
///     .add_model("sdxl", "stability-ai/sdxl:39ed52f2...")
///     .supported_types(vec![GenerationType::Image, GenerationType::Audio])
///     .build();
/// ```
#[derive(Debug)]
pub(crate) struct ReplicateProviderBuilder {
    pub(crate) api_key: String,
    pub(crate) endpoint: String,
    pub(crate) model_mappings: HashMap<String, String>,
    pub(crate) supported_types: Vec<GenerationType>,
    /// Per-request cap for the HTTP client. Defaults to this module's
    /// `DEFAULT_TIMEOUT_SECS`; the factory overrides it with the provider's
    /// `timeout_seconds` config knob.
    pub(crate) timeout_secs: u64,
}

impl ReplicateProviderBuilder {
    /// Create a new builder with the given API key
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model_mappings: HashMap::new(),
            supported_types: vec![GenerationType::Image, GenerationType::Audio],
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    /// Set the per-request HTTP timeout.
    #[must_use]
    pub const fn timeout_secs(mut self, secs: Option<u64>) -> Self {
        // `None` = unconfigured. Keep the default this builder chose; the
        // config field cannot express "unset" any other way.
        if let Some(secs) = secs {
            self.timeout_secs = secs;
        }
        self
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
    #[must_use]
    pub fn supported_types(mut self, types: Vec<GenerationType>) -> Self {
        self.supported_types = types;
        self
    }

    /// Build the `ReplicateProvider`
    ///
    /// Returns a provider even if the underlying `reqwest::Client` builder
    /// failed — the failure path silently substitutes a default-constructed
    /// client (no timeout, no other configuration). This mirrors the
    /// pre-fix behaviour and is retained for the unit-test call sites that
    /// only need an instance to exercise request-handling code.
    ///
    /// Production paths (the factory arm in
    /// `src/generation/providers/factory.rs`) must use [`Self::try_build`]
    /// so a TLS / DNS resolver misconfiguration surfaces at startup instead
    /// of producing a provider with no per-request timeout.
    #[cfg(test)]
    #[must_use]
    pub fn build(self) -> ReplicateProvider {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs.max(1)))
            .build()
            .unwrap_or_else(|e| {
                tracing::warn!(
                    subsystem = "generation",
                    provider = "replicate",
                    error = %e,
                    "reqwest::Client::builder() failed; substituting default client \
                     (no timeout). Production should use ReplicateProviderBuilder::try_build \
                     via the factory arm."
                );
                Client::new()
            });

        ReplicateProvider {
            client,
            api_key: self.api_key,
            endpoint: self.endpoint,
            model_mappings: self.model_mappings,
            supported_types: self.supported_types,
        }
    }

    /// Build the `ReplicateProvider`, propagating reqwest client-build errors
    /// to the caller.
    ///
    /// Every other builder in this module (`with_timeout`,
    /// `generation_http_client`, `voice_http_client`) propagates the error.
    /// The Replicate builder's `unwrap_or_default()` was the only one that
    /// silently swallowed it, masking a process-startup misconfiguration.
    /// Production callers (factory arms) must use this method; the
    /// infallible [`Self::build`] is retained for unit tests that exercise
    /// request-handling logic and never actually dispatch the HTTP client.
    pub fn try_build(self) -> Result<ReplicateProvider, GenerationError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs.max(1)))
            .build()
            .map_err(|e| {
                GenerationError::network(format!(
                    "replicate: failed to build HTTP client: {e}"
                ))
            })?;

        Ok(ReplicateProvider {
            client,
            api_key: self.api_key,
            endpoint: self.endpoint,
            model_mappings: self.model_mappings,
            supported_types: self.supported_types,
        })
    }
}

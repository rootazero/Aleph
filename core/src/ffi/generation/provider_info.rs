//! FFI-safe provider information types
//!
//! This module contains types for provider configuration and information
//! exposed through the FFI boundary.

use super::types::GenerationTypeFFI;

/// FFI-safe provider info for listing
#[derive(Debug, Clone)]
pub struct GenerationProviderInfoFFI {
    pub name: String,
    pub color: String,
    pub supported_types: Vec<GenerationTypeFFI>,
    pub default_model: Option<String>,
}

/// FFI-safe generation provider configuration
///
/// Used for adding/updating generation providers from the UI.
#[derive(Debug, Clone)]
pub struct GenerationProviderConfigFFI {
    /// Provider type identifier (openai, openai_compat, stability, elevenlabs, etc.)
    pub provider_type: String,
    /// API key (optional, can use keychain)
    pub api_key: Option<String>,
    /// Base URL for API (optional, for self-hosted or proxy)
    pub base_url: Option<String>,
    /// Default model to use
    pub model: Option<String>,
    /// Whether this provider is enabled
    pub enabled: bool,
    /// Brand color for UI theming (hex format)
    pub color: String,
    /// Supported generation types
    pub capabilities: Vec<GenerationTypeFFI>,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
}

impl From<GenerationProviderConfigFFI> for crate::config::GenerationProviderConfig {
    fn from(ffi: GenerationProviderConfigFFI) -> Self {
        crate::config::GenerationProviderConfig {
            provider_type: ffi.provider_type,
            api_key: ffi.api_key,
            base_url: ffi.base_url,
            model: ffi.model,
            enabled: ffi.enabled,
            color: ffi.color,
            capabilities: ffi.capabilities.into_iter().map(|t| t.into()).collect(),
            timeout_seconds: ffi.timeout_seconds,
            defaults: Default::default(),
            models: Default::default(),
        }
    }
}

impl From<crate::config::GenerationProviderConfig> for GenerationProviderConfigFFI {
    fn from(config: crate::config::GenerationProviderConfig) -> Self {
        GenerationProviderConfigFFI {
            provider_type: config.provider_type,
            api_key: config.api_key,
            base_url: config.base_url,
            model: config.model,
            enabled: config.enabled,
            color: config.color,
            capabilities: config.capabilities.into_iter().map(|t| t.into()).collect(),
            timeout_seconds: config.timeout_seconds,
        }
    }
}

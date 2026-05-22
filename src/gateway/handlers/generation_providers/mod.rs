//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use crate::config::types::generation::presets::get_merged_generation_preset;
use crate::config::types::generation::GenerationProviderConfig;
use crate::config::Config;
use crate::gateway::security::SharedTokenManager;
use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};

/// Generation provider entry for RPC responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderEntry {
    pub name: String,
    pub config: GenerationProviderConfig,
    pub is_default_for: Vec<GenerationType>,
}

/// Test connection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

pub mod handlers;
pub mod helpers;
pub mod voices;

pub use handlers::*;
pub use voices::*;

use super::normalize_optional_string;

fn save_config(cfg: &Config) -> Result<(), String> {
    cfg.save_incremental(&["generation"])
        .map_err(|e| e.to_string())
}

/// Vault key prefix for generation provider API keys
fn vault_key(provider_name: &str) -> String {
    format!("gen:{}", provider_name)
}

/// Resolve API key from vault for a generation provider
fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(name), vault)
}

pub(crate) fn build_generation_provider_for_persistence(
    provider_name: &str,
    config: GenerationProviderConfig,
    generation_overrides: &crate::config::presets_override::GenerationPresetsOverride,
) -> GenerationProviderConfig {
    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset =
        get_merged_generation_preset(provider_name, &config.provider_type, generation_overrides);

    let base_url = match normalize_optional_string(config.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().and_then(|p| p.base_url.clone()),
    };

    let models = {
        let non_empty: Vec<String> = config
            .models
            .iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if non_empty.is_empty() {
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| vec![p.default_model.clone()])
                .unwrap_or_default()
        } else {
            non_empty
        }
    };

    GenerationProviderConfig {
        base_url,
        models,
        ..config
    }
}

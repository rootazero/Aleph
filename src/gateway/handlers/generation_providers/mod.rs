//! Generation Providers RPC handlers
//!
//! Provides RPC methods for managing generation providers (image, video, audio, speech).

use crate::config::types::generation::presets::get_merged_generation_preset;
use crate::config::types::generation::GenerationProviderConfig;
use crate::config::Config;
use crate::gateway::security::SharedTokenManager;
use serde::{Deserialize, Serialize};

// The response row used to be a local `GenerationProviderEntry` here, wrapping
// the config type directly and then having `has_api_key` / `generation_type`
// welded onto its serialized form. It is now
// `aleph_protocol::providers::GenerationProviderRow`, built in `wire.rs`, so
// the Panel compiles against the same struct instead of a hand copy that was
// missing `model_aliases`. Nothing consumed the local one afterwards, and a
// `pub` struct with no producer and no consumer is invisible to dead-code
// analysis — so it is deleted rather than left for someone to "fix" a field on.

/// Test connection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

pub mod handlers;
pub mod helpers;
pub mod voices;
pub mod wire;

pub use handlers::*;
pub use voices::*;

use super::normalize_optional_string;

fn save_config(cfg: &Config) -> Result<(), String> {
    cfg.save_incremental(&["generation"])
        .map_err(|e| e.to_string())
}

/// Vault key prefix for generation provider API keys
fn vault_key(provider_name: &str) -> String {
    format!("gen:{provider_name}")
}

/// Resolve API key from vault for a generation provider
fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(name), vault)
}

/// Build the `GenerationProviderConfig` to persist for a create/update RPC.
///
/// `prior` is the currently-stored entry on `generation_providers.update`, and
/// `None` on `.create`. It exists because the wire type
/// ([`aleph_protocol::providers::GenerationProviderConfigJson`]) is a lossy
/// view: rebuilding the struct from a payload alone **erased** every field the
/// wire cannot express, both in memory and — via `save_config` — on disk.
///
/// `model_aliases` is the one that does it today. An operator sets
/// `[generation.providers.x.model_aliases]` in `config.toml`, later toggles
/// anything at all from the Panel — a colour, a timeout, the enabled switch —
/// and the aliases are gone, with nothing logged and no error anywhere. The
/// chat family had the identical defect and it reached `cache_retention`,
/// where the only symptom was the bill; this is the same shape one subsystem
/// over (判据 §16).
///
/// Starting from `prior` and overwriting only what the payload can express
/// also means a field added to `GenerationProviderConfig` later is carried
/// forward by default instead of silently reset until someone notices.
///
/// **Known limit:** the wire has no way to say "clear the aliases" — an empty
/// incoming map reads as "the client could not express this", so it keeps
/// `prior`'s. Clearing them is a `config.toml` edit until the wire grows a
/// representation for it.
pub(crate) fn build_generation_provider_for_persistence(
    provider_name: &str,
    config: GenerationProviderConfig,
    generation_overrides: &crate::config::presets_override::GenerationPresetsOverride,
    prior: Option<&GenerationProviderConfig>,
) -> GenerationProviderConfig {
    // Exhaustive destructuring, not field access: a field added to
    // `GenerationProviderConfig` stops this function compiling, which is the
    // moment to decide whether the payload carries it or `prior` protects it.
    // Field access would have compiled fine and dropped it.
    let GenerationProviderConfig {
        provider_type,
        api_key,
        base_url: incoming_base_url,
        models: incoming_models,
        enabled,
        color,
        capabilities,
        timeout_seconds,
        defaults,
        model_aliases: incoming_aliases,
        verified,
        edit_url,
        voices_url,
    } = config;

    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_generation_preset(provider_name, &provider_type, generation_overrides);

    let base_url = match normalize_optional_string(incoming_base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().and_then(|p| p.base_url.clone()),
    };

    let models = {
        let non_empty: Vec<String> = incoming_models
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

    // Baseline: the stored entry when updating, so wire-inexpressible fields
    // survive; a fresh all-unset config on create.
    let mut out = prior.cloned().unwrap_or_default();

    out.model_aliases = if incoming_aliases.is_empty() {
        out.model_aliases
    } else {
        incoming_aliases
    };
    out.provider_type = provider_type;
    out.api_key = api_key;
    out.base_url = base_url;
    out.models = models;
    out.enabled = enabled;
    out.color = color;
    out.capabilities = capabilities;
    // Straight assignment, deliberately NOT the `model_aliases` rule above.
    // `None` here is a value the client can mean -- it is the Panel's "Auto"
    // checkbox, which saves by omitting the key -- so "absent" must be able to
    // erase a stored number. `model_aliases` is the opposite case: the wire has
    // no way to say "clear them", so empty can only mean "not mine to touch".
    // Giving this field the same treatment would make Auto unsettable once a
    // number had been saved, which is the defect this round exists to close.
    out.timeout_seconds = timeout_seconds;
    out.defaults = defaults;
    out.verified = verified;
    out.edit_url = edit_url;
    out.voices_url = voices_url;
    out
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::config::presets_override::GenerationPresetsOverride;
    use std::collections::HashMap;

    fn stored_with_aliases() -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: "openai".into(),
            models: vec!["dall-e-3".into()],
            color: "#10a37f".into(),
            model_aliases: HashMap::from([("fast".to_string(), "dall-e-2".to_string())]),
            ..Default::default()
        }
    }

    /// The defect this argument exists for, asserted by effect: a payload that
    /// changes an unrelated field must not take the aliases with it.
    ///
    /// Falsification: pass `None` as `prior` — the same call the handlers made
    /// before this round — and the assertion fails.
    #[test]
    fn an_unrelated_edit_does_not_erase_aliases_the_wire_cannot_express() {
        let prior = stored_with_aliases();

        // What the Panel sends after the operator changes only the colour.
        let payload = GenerationProviderConfig {
            provider_type: "openai".into(),
            models: vec!["dall-e-3".into()],
            color: "#ff0000".into(),
            model_aliases: HashMap::new(),
            ..Default::default()
        };

        let saved = build_generation_provider_for_persistence(
            "my-dalle",
            payload,
            &GenerationPresetsOverride::default(),
            Some(&prior),
        );

        assert_eq!(saved.color, "#ff0000", "the edit itself must land");
        assert_eq!(
            saved.model_aliases, prior.model_aliases,
            "aliases the wire cannot express must survive the save"
        );
    }

    /// The other half of the merge rule, and the one a reader is most likely to
    /// "fix" into the alias treatment: an absent `timeout_seconds` must ERASE a
    /// stored number, because absent is how the Panel's Auto checkbox says
    /// "use the provider's own default".
    ///
    /// Both directions are asserted. Only checking that Auto clears the number
    /// would stay green if the field were ignored outright, and only checking
    /// that a number lands would stay green under the alias rule — the exact
    /// change that would make Auto unsettable once any number had been saved.
    #[test]
    fn an_omitted_timeout_clears_a_stored_one_while_a_number_still_lands() {
        let stored_two_minutes = GenerationProviderConfig {
            provider_type: "openai".into(),
            timeout_seconds: Some(120),
            ..Default::default()
        };

        // Auto: the wire omits the key, serde gives None.
        let auto = build_generation_provider_for_persistence(
            "my-dalle",
            GenerationProviderConfig {
                provider_type: "openai".into(),
                timeout_seconds: None,
                ..Default::default()
            },
            &GenerationPresetsOverride::default(),
            Some(&stored_two_minutes),
        );
        assert_eq!(
            auto.timeout_seconds, None,
            "checking Auto must clear the stored number, not keep it"
        );

        // And the opposite direction, from a stored Auto to an explicit cap.
        let stored_auto = GenerationProviderConfig {
            provider_type: "openai".into(),
            timeout_seconds: None,
            ..Default::default()
        };
        let explicit = build_generation_provider_for_persistence(
            "my-dalle",
            GenerationProviderConfig {
                provider_type: "openai".into(),
                timeout_seconds: Some(30),
                ..Default::default()
            },
            &GenerationPresetsOverride::default(),
            Some(&stored_auto),
        );
        assert_eq!(
            explicit.timeout_seconds,
            Some(30),
            "an explicit cap must still land over a stored Auto"
        );
    }

    /// A client that CAN express aliases still wins over the stored ones.
    #[test]
    fn a_payload_that_carries_aliases_replaces_them() {
        let prior = stored_with_aliases();
        let payload = GenerationProviderConfig {
            provider_type: "openai".into(),
            model_aliases: HashMap::from([("cheap".to_string(), "dall-e-2".to_string())]),
            ..Default::default()
        };
        let saved = build_generation_provider_for_persistence(
            "my-dalle",
            payload,
            &GenerationPresetsOverride::default(),
            Some(&prior),
        );
        assert_eq!(saved.model_aliases.len(), 1);
        assert!(saved.model_aliases.contains_key("cheap"));
    }

    /// Create has no prior, and must not invent one.
    #[test]
    fn create_starts_from_a_fresh_config() {
        let saved = build_generation_provider_for_persistence(
            "brand-new",
            GenerationProviderConfig {
                provider_type: "openai".into(),
                ..Default::default()
            },
            &GenerationPresetsOverride::default(),
            None,
        );
        assert!(saved.model_aliases.is_empty());
    }
}

//! Provider presets registry
//!
//! Contains default configurations for known AI providers.
//!
//! The `ProviderPreset` struct mirrors hermes-agent's `ProviderProfile`
//! dataclass — a single declarative source of truth per provider that
//! collapses what used to be split across `PRESETS` and `PRESET_METADATA`.
//! Existing entries keep their 4-field shape; new fields default safely and
//! are opt-in via the `with_*` const builders.

use crate::providers::metadata::{Modality, ProviderMetadata};

mod registry;

#[cfg(test)]
mod tests;

pub use registry::{canonical_preset_id, canonical_profiles, PRESETS, PRESET_METADATA};

/// Temperature handling for providers with non-standard expectations.
///
/// Examples:
///   - Kimi-for-coding server-manages temperature → `TemperaturePolicy::Omit`
///   - GPT-5 family forbids non-default temperature → `Force(1.0)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TemperaturePolicy {
    /// Strip the `temperature` field from the outgoing request entirely.
    Omit,
    /// Override caller's value with this fixed temperature.
    Force(f32),
}

/// Provider preset configuration
///
/// Declarative profile for a built-in provider. The first four fields are
/// required (preserved for backwards compatibility with `merge_provider_preset`
/// and pre-hermes-parity call sites); the remaining fields are zero-cost
/// extensions that opt in via the `with_*` const builders.
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    // ── Required identity / routing ──────────────────────────────────────────
    /// Default base URL for the provider
    pub base_url: &'static str,
    /// Protocol to use (e.g., "openai", "anthropic")
    pub protocol: &'static str,
    /// Default color for UI
    pub color: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,

    // ── hermes-parity extensions (all opt-in, zero-cost defaults) ────────────
    /// Alternative names this preset answers to (case-insensitive).
    /// Eliminates duplicate `m.insert()` calls per name variant.
    pub aliases: &'static [&'static str],
    /// Curated fallback model IDs. First entry is the recommended fallback
    /// and must equal `default_model` (drift-guarded). This chain is both
    /// the picker roster and the failover walk order *within* the provider:
    /// `deps_builder::provider_chain` appends any rungs the operator did not
    /// list to the provider's failover model catalog.
    pub fallback_models: &'static [&'static str],
    /// Cheap auxiliary model for memory summarization, vision, classification.
    /// `None` → the preset has no cheap tier; consumers (summary routing)
    /// deliberately do **not** fall back to `default_model` — background jobs
    /// must not silently run on the flagship.
    pub default_aux_model: Option<&'static str>,
    /// Override for the `/models` discovery endpoint.
    /// `None` → use `{base_url}/models` (or protocol-specific default).
    pub models_url: Option<&'static str>,
    /// Vendor signup / API-key page (panel picker links to this).
    pub signup_url: Option<&'static str>,
    /// Human-friendly display name; falls back to canonical name when None.
    pub display_name: Option<&'static str>,
    /// One-line description shown under `display_name`.
    pub description: Option<&'static str>,
    /// Non-standard temperature handling (Kimi server-managed, GPT-5 fixed, …).
    /// `None` → pass caller's temperature through unchanged.
    pub temperature_policy: Option<TemperaturePolicy>,
    /// Whether `/models` probing is safe (some providers 404 or rate-limit).
    pub supports_health_check: bool,
    /// Modalities this preset can serve. Defaults to `&[Modality::Chat]`.
    /// Chat is the dominant modality for `src/providers/`; multi-modal entries
    /// (e.g. providers that also expose image/video) opt in via
    /// `.with_modalities(&[..])`.
    pub modalities: &'static [Modality],
    /// Vendor docs / landing page URL. Distinct from `signup_url` which
    /// points at the API-key creation flow.
    pub homepage: Option<&'static str>,
    /// This provider serves no fixed roster — the operator must name a model.
    ///
    /// A handful of presets are pure relays or per-deployment hosts (Replicate,
    /// Lepton, Anyscale, T8Star): there is no id Aleph could ship that would be
    /// right for an arbitrary account. They carried `default_model: ""`, which
    /// reads as "the default is the empty string" — `list_models` silently
    /// skipped them, the catalog rendered a blank cell, and selecting one would
    /// have put `model: ""` on the wire. This flag states the fact instead, so
    /// the surfaces can say "pick a model" and the drift guard can tell a
    /// deliberate blank from a forgotten one.
    pub requires_explicit_model: bool,
}

/// Default modality for chat-side presets — used when no override is set.
/// Exposed for the const fn ctor so we keep a single literal of this slice.
const DEFAULT_CHAT_MODALITIES: &[Modality] = &[Modality::Chat];

impl Default for ProviderPreset {
    fn default() -> Self {
        Self {
            base_url: "",
            protocol: "openai",
            color: "#888888",
            default_model: "",
            aliases: &[],
            fallback_models: &[],
            default_aux_model: None,
            models_url: None,
            signup_url: None,
            display_name: None,
            description: None,
            temperature_policy: None,
            supports_health_check: true,
            modalities: DEFAULT_CHAT_MODALITIES,
            homepage: None,
            requires_explicit_model: false,
        }
    }
}

impl ProviderPreset {
    /// Const factory for the required four-tuple; extensions stay defaulted.
    #[must_use]
    pub const fn new(
        base_url: &'static str,
        protocol: &'static str,
        color: &'static str,
        default_model: &'static str,
    ) -> Self {
        Self {
            base_url,
            protocol,
            color,
            default_model,
            aliases: &[],
            fallback_models: &[],
            default_aux_model: None,
            models_url: None,
            signup_url: None,
            display_name: None,
            description: None,
            temperature_policy: None,
            supports_health_check: true,
            modalities: DEFAULT_CHAT_MODALITIES,
            homepage: None,
            requires_explicit_model: false,
        }
    }

    #[must_use]
    pub const fn with_aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    #[must_use]
    pub const fn with_fallback_models(mut self, models: &'static [&'static str]) -> Self {
        self.fallback_models = models;
        self
    }

    #[must_use]
    pub const fn with_aux_model(mut self, model: &'static str) -> Self {
        self.default_aux_model = Some(model);
        self
    }

    #[must_use]
    pub const fn with_signup(mut self, url: &'static str) -> Self {
        self.signup_url = Some(url);
        self
    }

    #[must_use]
    pub const fn with_display(mut self, name: &'static str) -> Self {
        self.display_name = Some(name);
        self
    }

    #[must_use]
    pub const fn with_description(mut self, desc: &'static str) -> Self {
        self.description = Some(desc);
        self
    }

    #[must_use]
    pub const fn with_models_url(mut self, url: &'static str) -> Self {
        self.models_url = Some(url);
        self
    }

    #[must_use]
    pub const fn with_temperature_policy(mut self, p: TemperaturePolicy) -> Self {
        self.temperature_policy = Some(p);
        self
    }

    #[must_use]
    pub const fn no_health_check(mut self) -> Self {
        self.supports_health_check = false;
        self
    }

    #[must_use]
    pub const fn with_homepage(mut self, url: &'static str) -> Self {
        self.homepage = Some(url);
        self
    }

    #[must_use]
    pub const fn with_modalities(mut self, modalities: &'static [Modality]) -> Self {
        self.modalities = modalities;
        self
    }

    /// Mark this preset as bring-your-own-model: it ships no `default_model`
    /// because no single id would be right for an arbitrary account.
    #[must_use]
    pub const fn requires_explicit_model(mut self) -> Self {
        self.requires_explicit_model = true;
        self
    }

    /// Resolve `/models` discovery URL, falling back to `{base_url}/models`.
    #[must_use]
    pub fn resolve_models_url(&self) -> String {
        match self.models_url {
            Some(u) => u.to_string(),
            None => format!("{}/models", self.base_url.trim_end_matches('/')),
        }
    }
}

/// Look up rich metadata for a preset by name (case-insensitive).
///
/// Returns `None` only when the preset itself is unknown — every existing
/// preset now derives its metadata from its own `ProviderPreset` fields
/// (`display_name` / description / homepage / modalities), so the parallel
/// `PRESET_METADATA` map is just a cached view onto the same data.
pub fn provider_metadata(name: &str) -> Option<&'static ProviderMetadata> {
    PRESET_METADATA.get(name.to_lowercase().as_str())
}

/// All preset names (sorted) that serve the requested modality.
///
/// Reads directly from `preset.modalities` — opt-out of the chat default
/// is via `.with_modalities(&[..])` at the registry level.
///
/// Iterates the **canonical** profiles only: aliases are resolution keys,
/// not display rows (see [`canonical_profiles`]), so `kimi`/`moonshot` no
/// longer show up as two entries in pickers and `list_models`.
pub fn presets_by_modality(modality: Modality) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = registry::canonical_profiles()
        .iter()
        .filter(|(_, preset)| preset.modalities.contains(&modality))
        .map(|(name, _)| *name)
        .collect();
    out.sort_unstable();
    out
}

/// Get a preset by name (case-insensitive). Walks aliases on miss.
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    let lower = name.to_lowercase();
    PRESETS.get(lower.as_str())
}

/// Get a preset with override support.
///
/// Resolution order:
/// 1. If a user override exists for the name (or via alias), merge it onto the built-in preset.
/// 2. If only a built-in preset exists, convert it to owned form.
/// 3. If only a user override exists (new provider), create from partial.
/// 4. Returns `None` if disabled or not found.
pub fn get_merged_preset(
    name: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedProviderPreset> {
    let lower = name.to_lowercase();
    let builtin = PRESETS.get(lower.as_str());
    let partial = overrides.providers.get(&lower).or_else(|| {
        // Check aliases in user overrides
        overrides
            .providers
            .values()
            .find(|p| p.aliases.iter().any(|a| a.to_lowercase() == lower))
    });

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if !p.enabled {
                return None;
            }
            Some(crate::config::presets_override::merge_provider_preset(b, p))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedProviderPreset {
            base_url: b.base_url.to_string(),
            protocol: b.protocol.to_string(),
            color: b.color.to_string(),
            default_model: b.default_model.to_string(),
        }),
        (None, Some(p)) => {
            if !p.enabled {
                return None;
            }
            crate::config::presets_override::partial_to_provider_preset(p)
        }
        (None, None) => None,
    }
}

/// Apply a preset's temperature policy to a caller-supplied temperature.
///
/// Returns the temperature value the provider request body should carry:
/// * `policy = None`             → `raw` unchanged (pass-through)
/// * `policy = Some(Omit)`       → `None` (drop the field from the request)
/// * `policy = Some(Force(f))`   → `Some(f)` (override caller's value)
///
/// Adapters look the policy up via [`temperature_for_base_url`] using
/// `config.base_url`, which they already have on hand. Custom `base_urls`
/// (user override) silently miss the lookup, which is the right default —
/// users overriding `base_url` have opted out of preset assumptions.
#[must_use]
pub const fn apply_temperature_policy(
    policy: Option<TemperaturePolicy>,
    raw: Option<f32>,
) -> Option<f32> {
    match policy {
        None => raw,
        Some(TemperaturePolicy::Omit) => None,
        Some(TemperaturePolicy::Force(value)) => Some(value),
    }
}

/// Resolve `temperature` for a request given the provider's `base_url`.
///
/// Convenience wrapper around [`apply_temperature_policy`] that does the
/// preset reverse-lookup so adapters don't need to thread preset names
/// through their plumbing. `base_url = None` and unknown URLs both fall
/// through with `raw` unchanged.
pub fn temperature_for_base_url(base_url: Option<&str>, raw: Option<f32>) -> Option<f32> {
    let url = match base_url {
        Some(u) => u,
        None => return raw,
    };
    let policy = registry::PRESETS_BY_BASE_URL
        .get(url)
        .and_then(|p| p.temperature_policy);
    apply_temperature_policy(policy, raw)
}

/// Resolve provider name from model name using known prefix patterns.
///
/// Delegates to [`crate::providers::model_catalog::infer_vendor`], the single
/// source of truth for model-name → vendor inference (hermes-parity prefix
/// table). The historical four outputs (openai/anthropic/google/deepseek)
/// are preserved; the catalog additionally recognises xai/mistral/moonshot/
/// qwen/etc. Callers guard the result with a registry membership check, so
/// the wider coverage is strictly additive.
pub fn resolve_provider_from_model(model: &str) -> Option<String> {
    crate::providers::model_catalog::infer_vendor(model).map(String::from)
}

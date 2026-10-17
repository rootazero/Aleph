//! The `providers.*` RPC contract — request params and response rows.
//!
//! # Why these types live here and not next to the handlers
//!
//! This family has **four** clients in **four** crates: the Panel
//! (`aleph-panel`), the TUI (`aleph-tui`), the CLI (`aleph-cli`) and the
//! handlers themselves (`alephcore`). `aleph-cli` and `aleph-tui` deliberately
//! **cannot** depend on `alephcore` — their `Cargo.toml`s say so in capitals.
//!
//! So before this module the wire shape was written four times, and the copies
//! disagreed in both directions:
//!
//! * `aleph providers list` renders a `type` column and a `default` column. The
//!   server has never emitted either name — it sends `provider_type` and
//!   `is_default` — so both columns printed a placeholder on every row since
//!   the command was written, and no test went red.
//! * `aleph providers add` sends a flat `{name, type, api_key, base_url}` body.
//!   `providers.create` takes `{name, config: {...}}` and its `models` field
//!   has no `#[serde(default)]`, so **every** invocation answered
//!   `INVALID_PARAMS`. The command had never once worked.
//! * The Panel's write DTO declared `model: String` where the server declares
//!   `models: Vec<String>`, so a provider carrying a model ladder was silently
//!   truncated to its first rung the moment anyone toggled its "enabled" switch
//!   from the settings page.
//!
//! Sharing one type makes a rename a **compile** error on every side at once.
//! That is the only guard that holds for a contract whose halves live in
//! different crates: a hand-copied fixture can be fooled by a field name that
//! merely looks right, and a client-side assertion against a literal the same
//! test just wrote is fooled by everything.
//!
//! ## Build responses from these types, do not merely parse them
//!
//! Deserialising a real response into a contract type only proves the response
//! is a **superset** — serde ignores unknown keys, so an over-sending server
//! stays invisible. `alephcore` therefore *constructs* `ProviderInfo` /
//! `CatalogEntry` / `ModelsRefreshRow` and serialises them, which makes
//! over-sending a compile-time impossibility rather than a test someone has to
//! remember to write.

use serde::{de, Deserialize, Serialize};

use super::catalog::{DiscoveredModel, RosterModel};

// ============================================================================
// Shared field helpers
// ============================================================================

/// Split a possibly comma-separated model string into individual model names.
///
/// The single-field provider form has always told users to separate multiple
/// models with commas, so the wire has always had to accept it. Model names
/// never contain commas, which is what makes this unambiguous.
fn split_csv_models(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The same tolerance as [`deserialize_models`], but an empty result is a
/// value rather than an error.
///
/// Generation providers are the case: a client may legitimately configure one
/// with no model at all (the Panel's "add custom provider" form sends
/// `models: []` when the field is blank, and the server then restores the
/// preset's default). Under the strict parser that payload does not lose the
/// model — it fails the **whole request** with `models list cannot be empty`,
/// because serde rejects the object, not the field.
///
/// It lives here, next to its strict sibling, because the generation config's
/// `models` field is parsed twice — once from `config.toml` by `alephcore` and
/// once from a JSON payload by [`super::generation::GenerationProviderConfigJson`]
/// — and two parsers with two tolerances for one field is the shape where a
/// value that survives the file gets rejected on the wire (判据 §1).
///
/// # Errors
/// Returns a deserializer error only when the value is neither null, a string,
/// nor a sequence of strings.
pub fn deserialize_optional_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct OptionalModelsVisitor;

    impl<'de> de::Visitor<'de> for OptionalModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string, array of strings, or null")
        }

        fn visit_none<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<Vec<String>, E> {
            Ok(Vec::new())
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            Ok(split_csv_models(value))
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                models.extend(split_csv_models(&s));
            }
            Ok(models)
        }
    }

    deserializer.deserialize_any(OptionalModelsVisitor)
}

/// Accepts `"a"`, `"a,b"`, `["a", "b"]`, or `["a,b"]`; rejects empty results.
///
/// The `["a,b"]` shape is not hypothetical — legacy tool writes stored a single
/// comma-joined element — so the sequence arm fans each element out too.
///
/// # Errors
/// Returns a deserializer error when the value is neither a string nor a
/// sequence of strings, or when it yields no non-empty model name.
pub fn deserialize_models<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: de::Deserializer<'de>,
{
    struct ModelsVisitor;

    impl<'de> de::Visitor<'de> for ModelsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<String>, E> {
            let models = split_csv_models(value);
            if models.is_empty() {
                Err(E::custom("model name cannot be empty"))
            } else {
                Ok(models)
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut models = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                models.extend(split_csv_models(&s));
            }
            if models.is_empty() {
                Err(de::Error::custom("models list cannot be empty"))
            } else {
                Ok(models)
            }
        }
    }

    deserializer.deserialize_any(ModelsVisitor)
}

fn default_provider_color() -> String {
    "#808080".to_string()
}

const fn default_timeout_seconds() -> u64 {
    300
}

// ============================================================================
// providers.list / providers.get
// ============================================================================

/// One configured provider, as returned by `providers.list` and
/// `providers.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// The provider's model ladder, in order. Element 0 is the model used when
    /// a turn names none; the rest are the failover rungs.
    #[serde(default)]
    pub models: Vec<String>,
    /// `models[0]`, or empty. A **projection** kept for older clients that read
    /// a single model — derived here from `models` in one place so the two can
    /// never disagree.
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub has_api_key: bool,
    /// Vault-side API key. **Never serialized.** This field is intentionally
    /// `#[serde(skip_serializing)]` after the 2026-XX-XX
    /// `/severed-wire-audit` review: the previous
    /// `Option<String>` + `skip_serializing_if = "Option::is_none"` was only
    /// a convention — any caller that built a `ProviderInfo` by copying a
    /// stored provider row would push a real API key onto the wire on every
    /// `providers.list` / `providers.get` response. The marker attribute
    /// turns the leak from a runtime convention into a compile-time
    /// guarantee: the field is writable for in-process construction but is
    /// physically absent from the serialized output. Clients that need the
    /// actual key must call the audited `provider.secret` RPC
    /// (`GenerationProviderConfigJson` is the request-side counterpart for
    /// writes).
    ///
    /// Inbound `serde_json` deserialization still accepts the field for
    /// backward compatibility with any pre-fix response already cached
    /// client-side; nothing on the receiving side relies on the value.
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default = "default_provider_color")]
    pub color: String,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Operator-declared total context window (tokens). `None` means "use the
    /// capability catalog".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub verified: bool,
    // ----- Fields added for per-model parameter plumbing (feat/per-model-params-core) -----
    // All are readback: the server pushes the current stored values so the
    // Panel can pre-fill accurately. They mirror `ProviderConfig` 1:1.
    /// Per-model override entries. Empty when the operator has not set any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_defs: Vec<ModelDefJson>,
    /// Gemini thinking level (e.g. `LOW` / `HIGH`); absent = use default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// Extended thinking budget hint (Anthropic) or `reasoning_effort` (OpenAI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `"off"` | `"short"` | `"long"` — None = provider default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<String>,
    /// Anthropic / OpenAI service tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Comma-separated stop sequences (Claude / Gemini / Ollama).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<String>,
    /// Gemini media resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_resolution: Option<String>,
    /// `"prepend"` | `"standard"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_mode: Option<String>,
    /// Behaviour profile override (see `model_behaviors/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_behavior: Option<String>,
    /// Anthropic `metadata.user_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_user_id: Option<String>,
    /// JSON-stringified `ResponseFormat` (the core enum is tagged; the wire
    /// keeps the editor simple).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    /// Streaming idle timeout override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_secs: Option<u64>,
    /// OpenAI `frequency_penalty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// OpenAI `presence_penalty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Ollama `repeat_penalty`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    /// Deterministic sampling seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// OpenAI `logprobs` toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    /// OpenAI `top_logprobs` count (0..=20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u8>,
    /// `parallel_tool_calls` capability gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

impl ProviderInfo {
    /// Keep the legacy `model` projection in step with `models`.
    ///
    /// Two representations of one fact drift the instant they get two authors,
    /// so the projection is derived here and nowhere else.
    #[must_use]
    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.model = models.first().cloned().unwrap_or_default();
        self.models = models;
        self
    }
}

// ============================================================================
// providers.create / update / test
// ============================================================================

/// Provider configuration as sent by a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    /// The model ladder. Accepts a single string or a CSV string for
    /// backwards compatibility (`alias = "model"`), but the ladder — not the
    /// first rung — is the contract.
    #[serde(deserialize_with = "deserialize_models", alias = "model")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Operator-declared total context window (tokens) — the escape hatch for
    /// third-party endpoints whose window differs from the capability catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    // ----- Fields added for per-model parameter plumbing (feat/per-model-params-core) -----
    /// Per-model overrides. `None` means "do not touch prior"; `Some(vec![])`
    /// means "explicitly clear"; `Some(non_empty)` means "set these".
    ///
    /// The `Option<Vec<_>>` shape is the cheapest way to express all three
    /// states on a single field — without it the persistence path cannot tell
    /// "client omitted the field" (preserve) from "client sent an empty list"
    /// (clear).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_defs: Option<Vec<ModelDefJson>>,
    /// Provider-level thinking level — see `ProviderInfo::thinking_level`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// Provider-level extended-thinking budget hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// One of `"off"` | `"short"` | `"long"`; None = use provider default.
    /// Unknown values are rejected by the handler as `INVALID_PARAMS` so a
    /// typo does not silently downgrade retention (the cache-bill bug
    /// `docs/reference/FEATURE_LOCATOR.md` calls out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_user_id: Option<String>,
    /// JSON-stringified `ResponseFormat`; the editor stays text for now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

impl ProviderConfigJson {
    /// A minimal enabled configuration carrying just a ladder.
    #[must_use]
    pub fn new(models: Vec<String>) -> Self {
        Self {
            protocol: None,
            enabled: true,
            models,
            api_key: None,
            base_url: None,
            color: None,
            timeout_seconds: None,
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            model_defs: None,
            thinking_level: None,
            effort: None,
            cache_retention: None,
            service_tier: None,
            stop_sequences: None,
            media_resolution: None,
            system_prompt_mode: None,
            model_behavior: None,
            metadata_user_id: None,
            response_format: None,
            stream_idle_timeout_secs: None,
            frequency_penalty: None,
            presence_penalty: None,
            repeat_penalty: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            parallel_tool_calls: None,
        }
    }
}

// ----------------------------------------------------------------------------
// Per-model overrides — wire projection of `alephcore::ProviderConfig::ModelDef`.
//
// Kept in the protocol crate because four clients share the shape and one of
// them (aleph-cli) cannot depend on alephcore. Each entry matches one model by
// `id` inside `ProviderConfig::model_defs` and wins over the capability catalog
// for the fields it sets. Unknown fields on the wire are tolerated by serde,
// which is what makes the legacy 11-field payload still parse against this
// expanded definition.
// ----------------------------------------------------------------------------

/// Input-modality capability flags for one model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelInputJson {
    pub text: bool,
    pub image: bool,
}

/// Tiered pricing row — applies when input tokens exceed `input_tokens_above`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostTierJson {
    #[serde(default)]
    pub input_tokens_above: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// Cost information for a model, in USD per million tokens. Mirrors the core
/// `ModelCost` so a panel-side edit writes straight through.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCostJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTierJson>,
}

/// Thinking-level → budget-token mapping. Wire form of core `ThinkingLevelMap`.
/// An empty string means "this level is not supported"; absent means "use the
/// provider default".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThinkingLevelMapJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xhigh: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
}

/// Compat flags for one model. All ten fields round-trip the core's `ModelCompat`
/// verbatim so a future adapter consumer can consume the wire without a second
/// schema. The Panel does not render them yet (the editor scaffold covers only
/// the basic fields the task asks for); the wire is here so manual JSON edits
/// through the Panel are not lossy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCompatJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uses_max_completion_tokens: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forbids_sampling_params: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_cache_control: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_object_properties: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_derefed_refs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_prompt_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_behavior_profile: Option<String>,
}

/// One per-model override entry. `id` is the model identifier from the
/// provider's ladder — keyed on the server side against `models[i]`. Every
/// other field is optional; absent means "inherit provider-level / catalog".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDefJson {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<ModelInputJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMapJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCostJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<ModelCompatJson>,
}

/// Parameters for `providers.get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetParams {
    pub name: String,
}

/// Parameters for `providers.create`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Parameters for `providers.update`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Parameters for `providers.delete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

/// Parameters for `providers.test`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestParams {
    /// Provider name — when present, a successful probe persists `verified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub config: ProviderConfigJson,
}

/// Parameters for `providers.setDefault`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDefaultParams {
    pub name: String,
}

/// Result of `providers.test`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// One row of the concurrent `providers.healthcheck` sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthRow {
    pub name: String,
    pub enabled: bool,
    /// True when a ping round-tripped successfully.
    pub ok: bool,
    /// True when the provider was not dialled at all, so `ok: false` says
    /// nothing about it.
    ///
    /// There are two reasons and `enabled` tells them apart without a third
    /// field: `!enabled` is the operator's own switch, `enabled` means the
    /// preset opts out of `/models` probing (OAuth-only endpoints,
    /// per-deployment hosts) and a probe could only ever fail there. Both
    /// come from `providers::probe::probe_disposition`, which is also what the
    /// `providers/connectivity` doctor check reads.
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of `providers.oauthLogin` and `providers.oauthStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub connected: bool,
    /// Canonical provider name the status speaks for — not always the name the
    /// caller asked about (`codex` resolves to `chatgpt`), which is exactly why
    /// it is echoed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// providers.catalog
// ============================================================================

/// Which slice of the preset catalogue to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CatalogView {
    /// Credentials present **and** verified.
    #[default]
    Configured,
    /// Credentials present, verified or not.
    Available,
    /// Every chat preset, even uncredentialed.
    All,
}

impl CatalogView {
    /// Stable wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Available => "available",
            Self::All => "all",
        }
    }
}

/// Parameters for `providers.catalog`.
///
/// Deliberately has no `query` / `limit` / `offset`: the catalogue is dozens of
/// rows, and both clients filter the rows they were sent through the shared
/// ranker in [`super::search`]. A server-side filter would be a second answer
/// to "which rows match" with no consumer that needs it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
}

/// Returned when `view` carries a string that is not a recognised
/// [`CatalogView`]. Surfaced as a JSON-RPC `INVALID_PARAMS` by the gateway
/// so a typo or an old client can no longer silently widen the listing to
/// `All` and reveal rows the caller never asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCatalogView(pub String);

impl std::fmt::Display for UnknownCatalogView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown catalog view: {:?} (expected one of: configured, available, all)",
            self.0
        )
    }
}

impl std::error::Error for UnknownCatalogView {}

impl CatalogParams {
    /// Parse `view`.
    ///
    /// Absent means `configured` (the narrow default a picker wants).
    /// An **unrecognised** string is a parse error: widening an unknown
    /// value to `All` would silently enlarge the response, including rows
    /// the caller did not ask to see. The gateway surfaces this as
    /// `INVALID_PARAMS` so the caller learns about the typo instead of
    /// quietly getting a different listing than they expected.
    pub fn view(&self) -> Result<CatalogView, UnknownCatalogView> {
        match self.view.as_deref() {
            None | Some("configured") => Ok(CatalogView::Configured),
            Some("available") => Ok(CatalogView::Available),
            Some("all") => Ok(CatalogView::All),
            Some(value) => Err(UnknownCatalogView(value.to_owned())),
        }
    }

    /// Build params for a given view.
    #[must_use]
    pub fn for_view(view: CatalogView) -> Self {
        Self {
            view: Some(view.as_str().to_string()),
        }
    }
}

/// How a provider is authenticated, so a client can group login-style presets
/// apart from key-style ones without keeping its own list of which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// Paste an API key.
    #[default]
    ApiKey,
    /// Sign in with the vendor (subscription login).
    OAuth,
}

/// One row in the chat-side provider/model catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Preset id — also the canonical config key.
    pub id: String,
    pub display_name: String,
    pub default_model: String,
    pub base_url: String,
    pub protocol: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Vendor signup / API-key creation URL — the "Get a key" call to action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_url: Option<String>,
    /// Alternative names this preset answers to (`moonshot` → `["kimi"]`).
    /// Searchable, so typing a vendor's other name still finds the row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub modalities: Vec<String>,
    /// The operator's configured ladder (`[providers.<id>] models`). Empty when
    /// the provider is not configured.
    #[serde(default)]
    pub models: Vec<String>,
    pub has_api_key: bool,
    pub verified: bool,
    pub enabled: bool,
    pub is_default: bool,
    /// How this provider is authenticated.
    #[serde(default)]
    pub auth_kind: AuthKind,
    /// `"local"` (on-machine / LAN) or `"cloud"` (public API). A fact about the
    /// *provider*, which is why it stays on the entry while the window, the
    /// price and the lifecycle moved down onto [`RosterModel`]: those three
    /// describe one model, and this row exists to let you pick among several.
    #[serde(default)]
    pub endpoint: String,
    /// This provider ships no default — the operator must name a model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub requires_explicit_model: bool,
    /// True when this provider exposes a live `/models` endpoint Aleph can
    /// read. False means "there is nothing to refresh here" — a client must
    /// offer a text entry instead of a refresh button, rather than offering a
    /// button that can only ever fail.
    #[serde(default)]
    pub discoverable: bool,
    /// The exact ids to offer, merged backend-side through the one ladder leaf
    /// the failover walk also uses. Rendered verbatim; never re-derived by a
    /// client (R4) — the merge includes an "operator moved `base_url` ⇒ no
    /// curated rungs" guard a frontend has no way to evaluate.
    #[serde(default)]
    pub roster: Vec<RosterModel>,
}

// ============================================================================
// providers.modelsRefresh
// ============================================================================

/// Parameters for `providers.modelsRefresh`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelsRefreshParams {
    /// Narrow the sweep to one provider id. Absent sweeps every enabled,
    /// credentialed provider concurrently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Why a provider's live model listing could not be fetched.
///
/// A single `error` string cannot answer the one question a client must answer
/// before it draws anything: *is this worth retrying?* "This vendor publishes
/// no catalogue endpoint" and "the request timed out" both used to arrive as
/// prose, so the only honest UI was the same sentence for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryFailureKind {
    /// The provider has no readable `/models` endpoint. Never retry; offer
    /// manual entry.
    Unsupported,
    /// No API key could be resolved. The user must link the provider first.
    MissingCredential,
    /// Network / TLS / DNS failure. Retryable.
    Transport,
    /// The endpoint answered with a non-success status (auth, rate limit).
    Status,
    /// The endpoint answered, but not in a shape we could parse.
    Shape,
    /// The request exceeded its deadline. Retryable.
    Timeout,
}

/// One provider's result in a `providers.modelsRefresh` sweep.
///
/// Per-provider failures are rows, never an RPC error: one unreachable vendor
/// must not blank the whole result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsRefreshRow {
    pub provider: String,
    /// True when a listing is present — including a stale one.
    pub ok: bool,
    /// The listing is the last known good snapshot, not a live answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    /// Unix seconds the listing was fetched. Absent when there is no listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<DiscoveredModel>,
    /// Present whenever the live fetch failed — on a stale row it explains why
    /// the data is dated rather than declaring the row a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DiscoveryFailureKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ModelsRefreshRow {
    /// What this row actually says, as one verdict.
    ///
    /// # Why this is not three `match (ok, stale)` expressions
    ///
    /// It was. The CLI, the TUI and the Panel each derived the tri-state from
    /// `(ok, stale)` in their own file, which is survivable only while the
    /// states never change: the moment a fourth one appears, "how did this turn
    /// out" has three independent answers and two of them are a round behind.
    /// Wording stays per-face (R4 — a terminal line, a table cell and a
    /// localized `<p>` are not the same sentence); the *verdict* is here.
    ///
    /// [`RefreshOutcome::NotApplicable`] is that fourth state: a preset that
    /// publishes no `/models` endpoint is not a failure to report in red, it is
    /// a question that does not apply to it. It is derived from `kind` rather
    /// than from a new wire field on purpose — adding a variant to an
    /// externally-tagged enum breaks old clients on the *whole row*, and this
    /// row already carried everything needed to tell the four apart.
    #[must_use]
    pub const fn outcome(&self) -> RefreshOutcome {
        if matches!(self.kind, Some(DiscoveryFailureKind::Unsupported)) && !self.ok {
            return RefreshOutcome::NotApplicable;
        }
        if !self.ok {
            return RefreshOutcome::Failed;
        }
        if self.stale {
            return RefreshOutcome::Stale;
        }
        RefreshOutcome::Live
    }
}

/// How one `providers.modelsRefresh` row turned out.
///
/// Not a wire type: derived from the row by [`ModelsRefreshRow::outcome`] so
/// every face reaches the same verdict from the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// A listing was fetched just now.
    Live,
    /// The listing shown is the last known good snapshot; the live fetch failed.
    Stale,
    /// No listing, and asking again might work.
    Failed,
    /// This endpoint publishes no model listing at all. Nothing failed; there
    /// was never anything to fetch. Offer manual entry, not a retry.
    NotApplicable,
}

/// Response of `providers.modelsRefresh`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelsRefreshResult {
    pub providers: Vec<ModelsRefreshRow>,
}

// ============================================================================
// Response envelopes
// ============================================================================
//
// The rows were the interesting part, so the single-key wrappers around them
// were left as `json!({ "items": … })` on the server and a `result.get("items")`
// on each client — which is the same hand-copied wire key this module exists to
// abolish, one level up. Three clients had each written it out separately.

/// Response of `providers.catalog`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogResult {
    pub items: Vec<CatalogEntry>,
}

/// Response of `providers.list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderListResult {
    pub providers: Vec<ProviderInfo>,
}

/// Response of `providers.healthcheck`.
///
/// The row type shipped here from the start; the envelope around it stayed a
/// hand-written `json!({ "providers": … })` because the method had no client to
/// disagree with it. It has one now, so the wrapper is a contract like the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderHealthResult {
    pub providers: Vec<ProviderHealthRow>,
}

/// Response of `providers.get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderGetResult {
    pub provider: ProviderInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_accepts_every_legacy_shape() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "deserialize_models", alias = "model")]
            models: Vec<String>,
        }

        let single: Holder = serde_json::from_str(r#"{"model":"a"}"#).unwrap();
        assert_eq!(single.models, vec!["a"]);

        let csv: Holder = serde_json::from_str(r#"{"model":"a, b"}"#).unwrap();
        assert_eq!(csv.models, vec!["a", "b"]);

        let list: Holder = serde_json::from_str(r#"{"models":["a","b"]}"#).unwrap();
        assert_eq!(list.models, vec!["a", "b"]);

        let nested: Holder = serde_json::from_str(r#"{"models":["a,b"]}"#).unwrap();
        assert_eq!(nested.models, vec!["a", "b"]);

        assert!(serde_json::from_str::<Holder>(r#"{"models":[]}"#).is_err());
        assert!(serde_json::from_str::<Holder>(r#"{"model":""}"#).is_err());
    }

    #[test]
    fn the_legacy_model_projection_is_the_first_rung() {
        let info = ProviderInfo {
            name: "openai".into(),
            enabled: true,
            models: Vec::new(),
            model: String::new(),
            provider_type: None,
            has_api_key: false,
            api_key: None,
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            max_tokens: None,
            context_window: None,
            temperature: None,
            is_default: false,
            verified: false,
            model_defs: Vec::new(),
            thinking_level: None,
            effort: None,
            cache_retention: None,
            service_tier: None,
            stop_sequences: None,
            media_resolution: None,
            system_prompt_mode: None,
            model_behavior: None,
            metadata_user_id: None,
            response_format: None,
            stream_idle_timeout_secs: None,
            frequency_penalty: None,
            presence_penalty: None,
            repeat_penalty: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            parallel_tool_calls: None,
        }
        .with_models(vec!["a".into(), "b".into()]);

        assert_eq!(info.model, "a");
        assert_eq!(info.models, vec!["a", "b"]);
    }

    #[test]
    fn an_absent_view_is_configured_and_an_unknown_one_errors() {
        assert_eq!(
            CatalogParams::default().view().unwrap(),
            CatalogView::Configured
        );
        assert_eq!(
            CatalogParams {
                view: Some("configured".into())
            }
            .view()
            .unwrap(),
            CatalogView::Configured
        );
        assert_eq!(
            CatalogParams::for_view(CatalogView::Available)
                .view()
                .unwrap(),
            CatalogView::Available
        );
        // Unknown values are now a parse error, NOT silently widened to `All`:
        // a typo or an old client used to enlarge the response to rows the
        // caller did not ask for. The gateway surfaces this as INVALID_PARAMS.
        assert_eq!(
            CatalogParams {
                view: Some("nonsense".into())
            }
            .view()
            .unwrap_err()
            .0,
            "nonsense"
        );
    }

    /// The new wire fields survive a serde round-trip with their values
    /// intact. This is the cheapest assertion that an old client sending only
    /// the legacy subset still parses (no required new field), and that a new
    /// client sending the new fields has them observable on the response.
    #[test]
    fn provider_config_round_trips_model_defs_and_per_model_params() {
        let original = ProviderConfigJson {
            protocol: Some("anthropic".into()),
            enabled: true,
            models: vec!["claude-opus-4-5".into(), "claude-sonnet-4-5".into()],
            api_key: None,
            base_url: Some("https://api.example.com/v1".into()),
            color: None,
            timeout_seconds: Some(600),
            max_tokens: Some(8192),
            context_window: Some(200_000),
            temperature: Some(0.7),
            top_p: Some(0.9),
            top_k: Some(40),
            model_defs: Some(vec![ModelDefJson {
                id: "claude-opus-4-5".into(),
                name: Some("Opus 4.5".into()),
                base_url: None,
                context_window: Some(200_000),
                max_output_tokens: Some(32_000),
                input: Some(ModelInputJson {
                    text: true,
                    image: true,
                }),
                reasoning: Some(true),
                thinking_level_map: Some(ThinkingLevelMapJson {
                    low: Some("low".into()),
                    medium: Some("medium".into()),
                    high: Some("high".into()),
                    ..ThinkingLevelMapJson::default()
                }),
                cost: Some(ModelCostJson {
                    input: Some(15.0),
                    output: Some(75.0),
                    cache_read: Some(1.5),
                    cache_write: Some(18.75),
                    reasoning: None,
                    tiers: vec![CostTierJson {
                        input_tokens_above: 200_000,
                        input: Some(30.0),
                        output: Some(150.0),
                        cache_read: Some(3.0),
                        cache_write: Some(37.5),
                    }],
                }),
                compat: Some(ModelCompatJson {
                    supports_cache_control: Some(true),
                    ..ModelCompatJson::default()
                }),
            }]),
            thinking_level: Some("medium".into()),
            effort: Some("high".into()),
            cache_retention: Some("long".into()),
            service_tier: Some("auto".into()),
            stop_sequences: Some("\n\nUser:".into()),
            media_resolution: None,
            system_prompt_mode: Some("standard".into()),
            model_behavior: None,
            metadata_user_id: Some("aleph-operator".into()),
            response_format: Some(r#"{"type":"text"}"#.into()),
            stream_idle_timeout_secs: Some(90),
            frequency_penalty: Some(0.0),
            presence_penalty: Some(0.0),
            repeat_penalty: None,
            seed: Some(42),
            logprobs: Some(false),
            top_logprobs: None,
            parallel_tool_calls: Some(true),
        };

        let json = serde_json::to_string(&original).expect("serializes");
        let decoded: ProviderConfigJson = serde_json::from_str(&json).expect("round-trips");

        assert_eq!(decoded.protocol, original.protocol);
        assert_eq!(decoded.models, original.models);
        assert_eq!(decoded.context_window, original.context_window);
        assert_eq!(decoded.cache_retention, original.cache_retention);
        assert_eq!(decoded.model_defs.as_ref().map(Vec::len), Some(1));
        let md = &decoded.model_defs.as_ref().unwrap()[0];
        assert_eq!(md.id, "claude-opus-4-5");
        assert_eq!(md.context_window, Some(200_000));
        assert_eq!(
            md.input.as_ref().map(|i| (i.text, i.image)),
            Some((true, true))
        );
        assert_eq!(
            md.cost
                .as_ref()
                .and_then(|c| c.tiers.first())
                .map(|t| t.input_tokens_above),
            Some(200_000)
        );
        assert_eq!(decoded.thinking_level.as_deref(), Some("medium"));
        assert_eq!(decoded.effort.as_deref(), Some("high"));
        assert_eq!(decoded.stream_idle_timeout_secs, Some(90));
        assert_eq!(decoded.seed, Some(42));
    }

    /// A legacy client that only knows the 11 original fields keeps working:
    /// every new field defaults to `None` and `model_defs` defaults to `None`
    /// (not an empty Vec — `None` is the "do not touch prior" sentinel the
    /// server uses, and it must round-trip from "absent" cleanly).
    #[test]
    fn legacy_payload_still_parses_with_defaults() {
        let legacy = r#"{
            "protocol": "openai",
            "enabled": true,
            "models": ["gpt-4o"],
            "api_key": null,
            "base_url": null,
            "color": null,
            "timeout_seconds": 300,
            "max_tokens": null,
            "context_window": 128000,
            "temperature": null,
            "top_p": null,
            "top_k": null
        }"#;
        let parsed: ProviderConfigJson = serde_json::from_str(legacy).expect("legacy parses");
        assert_eq!(parsed.protocol.as_deref(), Some("openai"));
        assert_eq!(parsed.context_window, Some(128_000));
        assert!(parsed.model_defs.is_none());
        assert!(parsed.thinking_level.is_none());
        assert!(parsed.effort.is_none());
        assert!(parsed.cache_retention.is_none());
        assert!(parsed.seed.is_none());
        assert!(parsed.parallel_tool_calls.is_none());
    }

    /// `cache_retention` accepts the three documented values and rejects
    /// anything else — silently mapping unknown strings to `Short` was the
    /// bug `docs/reference/FEATURE_LOCATOR.md` calls out as the cache-bill
    /// killer. The helper turns this into an `INVALID_PARAMS`; this test pins
    /// the wire-layer tolerance.
    #[test]
    fn cache_retention_accepts_the_three_documented_values() {
        for v in ["off", "short", "long"] {
            let json = format!(r#"{{"models":["a"],"cache_retention":"{v}"}}"#);
            let parsed: ProviderConfigJson =
                serde_json::from_str(&json).unwrap_or_else(|e| panic!("{v} parses: {e}"));
            assert_eq!(parsed.cache_retention.as_deref(), Some(v));
        }
    }

    /// Duplicate ids in a single `model_defs` payload pass through the wire
    /// unchanged — the helper is where the user-visible dedup error lives.
    /// This test pins the contract: the wire stays honest about what the
    /// client sent so the helper is the only layer that decides "first
    /// wins" vs "last wins" vs "reject".
    #[test]
    fn model_defs_with_duplicate_ids_is_well_formed() {
        let payload = r#"{
            "models": ["a"],
            "model_defs": [
                {"id": "a", "context_window": 100000},
                {"id": "a", "context_window": 200000}
            ]
        }"#;
        let parsed: ProviderConfigJson = serde_json::from_str(payload).expect("parses");
        let defs = parsed.model_defs.expect("present");
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].context_window, Some(100_000));
        assert_eq!(defs[1].context_window, Some(200_000));
    }
}

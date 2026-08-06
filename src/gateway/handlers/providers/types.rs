//! Provider handler types and parameter structs.

use serde::{Deserialize, Serialize};

/// Provider info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub models: Vec<String>,
    /// First model name (for panel backward compat)
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    pub has_api_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub color: String,
    pub timeout_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Operator-declared total context window (tokens). Surfaced so the panel /
    /// CLI can display and round-trip it through `providers.update`. `None`
    /// means "use the capability catalog" (model-aware budgeting falls back).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub is_default: bool,
    pub verified: bool,
}

/// Test result
#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl From<crate::providers::probe::ProbeOutcome> for TestResult {
    fn from(o: crate::providers::probe::ProbeOutcome) -> Self {
        Self {
            success: o.success,
            error: o
                .error
                .map(|error| crate::diagnostics::redact_secrets(&error)),
            latency_ms: o.latency_ms,
        }
    }
}

/// One row in the concurrent `providers.healthcheck` result.
///
/// Unlike `providers.test` (one provider, mutates `verified`), this is a
/// read-only liveness probe over *every* configured provider, run
/// concurrently. Disabled providers are reported as `skipped` rather than
/// probed.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealthRow {
    pub name: String,
    pub enabled: bool,
    /// True when a ping round-tripped successfully.
    pub ok: bool,
    /// True when the provider was not probed because it is disabled.
    pub skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for providers.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub name: String,
}

/// Parameters for providers.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Provider config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigJson {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        deserialize_with = "crate::config::types::serde_helpers::deserialize_models",
        alias = "model"
    )]
    pub models: Vec<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Operator-declared total context window (tokens) — escape hatch for
    /// third-party endpoints whose window differs from / is absent in the
    /// capability catalog. Round-tripped into `ProviderConfig.context_window`.
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

/// Parameters for providers.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub name: String,
    pub config: ProviderConfigJson,
}

/// Parameters for providers.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

/// Parameters for providers.test
#[derive(Debug, Deserialize)]
pub struct TestParams {
    /// Provider name (if provided, persist verified=true on success)
    #[serde(default)]
    pub name: Option<String>,
    pub config: ProviderConfigJson,
}

/// Parameters for providers.setDefault
#[derive(Debug, Deserialize)]
pub struct SetDefaultParams {
    pub name: String,
}

/// Parameters for providers.catalog
///
/// Mirrors openclaw's `models.list { view }` shape so the panel chat-window
/// model picker can ask either "what is usable right now" or "what could be
/// usable if I added a key".
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CatalogParams {
    /// Filter mode:
    /// * `configured` (default) — credentials present **and** verified
    /// * `available`            — credentials present (verified or not)
    /// * `all`                  — every chat preset, even uncredentialed
    #[serde(default)]
    pub view: Option<String>,
}

/// One row in the chat-side provider/model catalog returned to the panel.
///
/// The fields are a flat join of:
/// * `chat_presets::PRESETS[name]`             (built-in template — protocol/url/color)
/// * `chat_presets::PRESET_METADATA[name]`     (display strings, modalities, homepage)
/// * `config.providers[name]` if configured    (user-extended models list, key state)
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntryView {
    /// Preset id (also the canonical config key).
    pub id: String,
    pub display_name: String,
    pub default_model: String,
    pub base_url: String,
    pub protocol: String,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Vendor signup / API-key creation URL (preset-level hint for the
    /// "Get a key" CTA in the panel picker). Distinct from `homepage`,
    /// which points at the vendor's docs/landing page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signup_url: Option<String>,
    /// Curated fallback model IDs used by the picker when the user has
    /// not yet customised `models` and `/models` discovery has not run.
    /// Empty vec = no curated fallback (picker falls back to `default_model`).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fallback_models: Vec<String>,
    /// Cheap auxiliary model hint (e.g. for memory summarisation). `None`
    /// means callers should reuse `default_model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_aux_model: Option<String>,
    /// Aliases the preset answers to (e.g. `moonshot` → `["kimi"]`).
    /// Empty when the preset has no aliases. Surfaced so the panel picker
    /// can show alternative names without re-fetching per alias.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub aliases: Vec<String>,
    pub modalities: Vec<String>,
    /// User-extended model list (from `config.providers[name].models`). May
    /// be empty when the provider is not yet configured — in that case fall
    /// back to `default_model` for picker display.
    pub models: Vec<String>,
    pub has_api_key: bool,
    pub verified: bool,
    pub enabled: bool,
    pub is_default: bool,
    /// Capability metadata for `default_model` (context window, vision,
    /// tools, reasoning). `None` when the model family is not in the catalog.
    /// Lets the picker — and the LLM reasoning over `providers.catalog` —
    /// see what the default model can do without a side lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<crate::providers::ModelCapabilities>,
    /// Per-million-token cost summary for `default_model`. `None` when the
    /// model is not priced. Cost-at-a-glance for the model picker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::pricing::RateCard>,
    /// Endpoint locality of this provider's `base_url`: `"local"` (on-machine
    /// / LAN) or `"cloud"` (public API). Derived from the same classifier the
    /// route policy uses internally; surfaced here so the picker — and the LLM
    /// reasoning over `providers.catalog` — can prefer an on-machine model
    /// without a side lookup. Always present (absent/unparseable `base_url`
    /// conservatively classifies as `"cloud"`).
    pub endpoint: String,
    /// Lifecycle of `default_model`: `"active"` / `"preview"` /
    /// `"deprecated"`, with the vendor's successor when it has retired. Lets
    /// the picker warn instead of offering an id that now 400s.
    pub lifecycle: crate::providers::model_catalog::ModelLifecycle,
    /// This provider ships no default — the operator must name a model. The
    /// picker should prompt for one rather than rendering `default_model`'s
    /// empty string as if it were a choice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub requires_explicit_model: bool,
    /// The exact model ids the picker should offer for this provider,
    /// computed backend-side through the same `presets::model_ladder` leaf
    /// the failover walk merges through: operator `models` first (or
    /// `default_model` when none are listed), then unlisted curated
    /// `fallback_models` rungs — skipped entirely when the operator moved
    /// `base_url` off the preset, where curated ids would be opaque 400s.
    /// The picker renders this verbatim and must not re-derive it (R4).
    pub roster: Vec<String>,
}

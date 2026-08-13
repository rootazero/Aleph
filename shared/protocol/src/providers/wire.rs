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

use super::catalog::{DiscoveredModel, ModelCapabilities, ModelLifecycle, RateCard, RosterModel};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        }
    }
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

impl CatalogParams {
    /// Parse `view`.
    ///
    /// Absent means `configured` (the narrow default a picker wants).
    /// An **unrecognised** string means `all`, which is the behaviour this
    /// family has always had: a listing that answers with less than the caller
    /// asked for is a silent functional regression, and the caller is already
    /// past the admin gate.
    #[must_use]
    pub fn view(&self) -> CatalogView {
        match self.view.as_deref() {
            None | Some("configured") => CatalogView::Configured,
            Some("available") => CatalogView::Available,
            _ => CatalogView::All,
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
    /// Curated fallback ids the preset ships. Informational: the authoritative
    /// list to offer is [`CatalogEntry::roster`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_models: Vec<String>,
    /// Cheap auxiliary model hint. `None` means "reuse `default_model`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_aux_model: Option<String>,
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
    /// Capabilities of `default_model`. `None` when the family is not in the
    /// curated table (a freshly discovered id, typically).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// Per-million-token cost of `default_model`. `None` when unpriced —
    /// which is not the same as free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<RateCard>,
    /// `"local"` (on-machine / LAN) or `"cloud"` (public API).
    #[serde(default)]
    pub endpoint: String,
    /// Lifecycle of `default_model`.
    #[serde(default)]
    pub lifecycle: ModelLifecycle,
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
        }
        .with_models(vec!["a".into(), "b".into()]);

        assert_eq!(info.model, "a");
        assert_eq!(info.models, vec!["a", "b"]);
    }

    #[test]
    fn an_absent_view_is_configured_and_an_unknown_one_is_all() {
        assert_eq!(CatalogParams::default().view(), CatalogView::Configured);
        assert_eq!(
            CatalogParams {
                view: Some("configured".into())
            }
            .view(),
            CatalogView::Configured
        );
        // Widening, not narrowing: the pre-contract handler did the same, and
        // answering with fewer rows than before would be a silent regression.
        assert_eq!(
            CatalogParams {
                view: Some("nonsense".into())
            }
            .view(),
            CatalogView::All
        );
        assert_eq!(
            CatalogParams::for_view(CatalogView::Available).view(),
            CatalogView::Available
        );
    }
}

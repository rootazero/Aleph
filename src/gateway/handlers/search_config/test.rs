//! `search.test` — probe one backend and record the outcome.
//!
//! # Why this file has no provider names in it
//!
//! It used to be a nine-arm `match` on `provider_type`, each arm calling a
//! concrete `XxxProvider::new(...)` and carrying its own "API key is required
//! for X" copy — a second implementation of "build a provider from a config",
//! sitting next to `ProviderFactoryRegistry`, which exists to be the first
//! one. Two consequences, both live:
//!
//! * A tenth backend would get a provider, a factory, a config shape and a
//!   Panel card, and its Test button would answer `Unknown provider type` —
//!   the tail arm was the only thing that told you the two lists had drifted,
//!   and it said it to the operator rather than to the build.
//! * The arms restated the factories' construction rules (which field is
//!   mandatory for whom) from memory. `searxng` needing a `base_url` was
//!   written down in three places: the factory, the preset table the form is
//!   built from, and here.
//!
//! Now the factory builds it and `aleph_protocol::search`'s preset table
//! explains a refusal. What is left that is genuinely this file's own is the
//! probe (a one-result live search) and the `verified` bookkeeping.

use crate::config::types::SearchBackendConfig;
use crate::config::Config;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::search_config::dto::resolve_api_key;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::SharedTokenManager;
use crate::search::{ProviderFactoryRegistry, SearchOptions};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

impl SearchTestResult {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
        }
    }
}

/// Why the factory refused to build this backend, phrased for the operator
/// looking at the form.
///
/// Derived from the same preset table the form's fields come from, so the
/// sentence names a field that is actually on screen. The factory itself
/// answers `Ok(None)` for every refusal and logs the reason at WARN; that is
/// right for boot (one skipped backend must not abort the load) and useless
/// for a button whose entire job is to say what is wrong.
fn missing_requirement(provider_type: &str, backend: &SearchBackendConfig) -> Option<String> {
    let preset = aleph_protocol::search::preset(provider_type)?;
    let blank = |v: &Option<String>| v.as_deref().is_none_or(|s| s.trim().is_empty());
    if preset.needs_api_key && blank(&backend.api_key) {
        return Some(format!("API key is required for {}", preset.display_name));
    }
    if preset.needs_base_url && blank(&backend.base_url) {
        return Some(format!("Base URL is required for {}", preset.display_name));
    }
    if preset.needs_engine_id && blank(&backend.engine_id) {
        return Some(format!(
            "Search engine ID is required for {}",
            preset.display_name
        ));
    }
    None
}

/// Test a search backend connection
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Backend name (used to persist verified=true on success)
        name: String,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        engine_id: Option<String>,
        /// `SearXNG` only — comma-separated upstream engines to pin for the probe.
        #[serde(default)]
        engines: Option<String>,
    }

    let mut params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Resolve API key from vault if not provided inline
    if params.api_key.is_none() {
        params.api_key = resolve_api_key(&params.name, &vault);
    }

    // Determine provider type from config or fallback to name
    let provider_type = {
        let cfg = config.read().await;
        cfg.search
            .as_ref()
            .and_then(|s| s.backends.get(&params.name))
            .map_or_else(|| params.name.clone(), |b| b.provider_type.clone())
    };

    let backend = SearchBackendConfig {
        provider_type: provider_type.clone(),
        api_key: params.api_key,
        base_url: params.base_url,
        engine_id: params.engine_id,
        engines: params.engines,
        // The probe must not be spaced out by SearXNG's request throttle: a
        // button that sits for the tuned interval before its first request
        // reads as a hang, and one request cannot burst anything.
        min_request_interval_ms: Some(0),
        verified: false,
    };

    let test_result = probe(&backend).await;

    // Persist verified=true on success
    if test_result.success {
        let mut cfg = config.write().await;
        if let Some(search) = &mut cfg.search {
            if let Some(b) = search.backends.get_mut(&params.name) {
                b.verified = true;
                if let Err(e) = cfg.save_incremental(&["search"]) {
                    tracing::error!(error = %e, "Failed to save config after search test");
                }
            }
        }
    }

    match serde_json::to_value(test_result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize result: {e}"),
        ),
    }
}

/// Build the backend through the factory and ask it one real question.
///
/// Split out of the handler so the construction half is testable without a
/// `JsonRpcRequest`, a `Config` or a vault — see the census below, which runs
/// every preset through it and never reaches the network because every one of
/// them stops at a missing credential first.
async fn probe(backend: &SearchBackendConfig) -> SearchTestResult {
    let factories = ProviderFactoryRegistry::with_defaults();
    let provider = match factories.build("search.test", backend) {
        Ok(Some(p)) => p,
        Ok(None) => {
            // Two different refusals arrive here as the same `None`: a
            // config the operator has not finished, and a `provider_type`
            // no factory claims. Only the first is actionable, and saying
            // "unknown provider" about the second-guessed one would send
            // the operator looking for a typo they did not make.
            return SearchTestResult::failed(
                missing_requirement(&backend.provider_type, backend)
                    .unwrap_or_else(|| format!("Unknown provider type: {}", backend.provider_type)),
            );
        }
        Err(e) => return SearchTestResult::failed(format!("Failed to create provider: {e}")),
    };

    let opts = SearchOptions {
        max_results: 1,
        ..Default::default()
    };
    match provider.search("test", &opts).await {
        Ok(_) => SearchTestResult {
            success: true,
            message: "Connection successful".to_string(),
        },
        Err(e) => SearchTestResult::failed(format!("Search failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(provider_type: &str) -> SearchBackendConfig {
        SearchBackendConfig {
            provider_type: provider_type.to_string(),
            api_key: None,
            base_url: None,
            engine_id: None,
            engines: None,
            min_request_interval_ms: Some(0),
            verified: false,
        }
    }

    /// Every card the Panel offers has to be reachable by the button on it.
    ///
    /// The old nine-arm `match` made this a promise nobody checked: a backend
    /// added to the factory and the preset table but not to the arm list got
    /// `Unknown provider type`, and the only way to find out was to click it.
    /// The assertion is on the *phrase*, because that is the one answer this
    /// handler must never give about a backend it ships a card for.
    ///
    /// No network: every preset either needs a credential (so `probe` stops at
    /// `missing_requirement`) or is DuckDuckGo, which is excluded here and
    /// covered by `duckduckgo_constructs_without_credentials` in the factory.
    #[tokio::test]
    async fn no_advertised_backend_answers_unknown_provider_type() {
        for preset in aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS {
            if !(preset.needs_api_key || preset.needs_base_url || preset.needs_engine_id) {
                continue; // would go to the network; see doc comment
            }
            let result = probe(&blank(preset.name)).await;
            assert!(
                !result.success,
                "{}: a blank config must not pass",
                preset.name
            );
            assert!(
                !result.message.contains("Unknown provider type"),
                "`{}` is offered as a card but the test face does not recognise it: {}",
                preset.name,
                result.message
            );
        }
    }

    /// A refusal has to name the field the operator can fill in. "Failed" with
    /// no noun is the apology this codebase spends its notes avoiding.
    #[test]
    fn a_refusal_names_the_field_that_is_missing() {
        for preset in aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS {
            let backend = blank(preset.name);
            let msg = missing_requirement(preset.name, &backend);
            if preset.needs_api_key {
                let msg = msg.expect("a backend needing a key must say so");
                assert!(msg.contains("API key"), "{}: {msg}", preset.name);
                assert!(msg.contains(preset.display_name), "{}: {msg}", preset.name);
            } else if preset.needs_base_url {
                let msg = msg.expect("a backend needing a base_url must say so");
                assert!(msg.contains("Base URL"), "{}: {msg}", preset.name);
            } else if preset.needs_engine_id {
                let msg = msg.expect("a backend needing an engine_id must say so");
                assert!(msg.contains("engine ID"), "{}: {msg}", preset.name);
            } else {
                assert!(
                    msg.is_none(),
                    "{} requires nothing, so nothing is missing: {msg:?}",
                    preset.name
                );
            }
        }
    }

    /// Whitespace is not a credential. The form posts what is in the box, and
    /// a box holding a space used to reach `TavilyProvider::new`, which only
    /// rejected the empty string — so the probe went to the network with a
    /// blank key and reported an auth failure instead of an unfilled field.
    #[test]
    fn an_all_whitespace_credential_is_treated_as_absent() {
        let mut backend = blank("tavily");
        backend.api_key = Some("   ".to_string());
        assert!(missing_requirement("tavily", &backend).is_some());
    }

    /// A `provider_type` no factory claims is the one case where "unknown" is
    /// the true answer, and it must still be reported rather than swallowed.
    #[tokio::test]
    async fn a_genuinely_unknown_provider_type_still_says_so() {
        let result = probe(&blank("brrrrave")).await;
        assert!(!result.success);
        assert!(
            result.message.contains("Unknown provider type"),
            "{}",
            result.message
        );
    }
}

use crate::error::{AlephError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult, WebFetchSerpFallback};
use crate::sync_primitives::Arc;

/// Map an `AlephError` returned by a search provider to a short, stable
/// kind label used for structured log output. Lets ops grep the search
/// log by failure mode (`kind=auth`, `kind=rate-limit`, ...) without
/// having to parse free-form error messages.
pub(super) const fn classify_search_error(e: &AlephError) -> &'static str {
    match e {
        AlephError::AuthenticationError { .. } => "auth",
        AlephError::RateLimitError { .. } => "rate-limit",
        AlephError::Timeout { .. } => "timeout",
        AlephError::NetworkError { .. } => "network",
        AlephError::Cancelled => "cancelled",
        AlephError::Validation(_) | AlephError::InvalidConfig { .. } => "config",
        AlephError::ProviderError { .. } => "provider",
        _ => "other",
    }
}
/// Search provider registry and router
///
/// This module manages multiple search providers and routes requests
use std::collections::HashMap;

/// Registry for managing multiple search providers
///
/// Maintains a pool of configured providers and routes search requests
/// to the appropriate backend based on configuration.
pub struct SearchRegistry {
    providers: HashMap<String, Arc<dyn SearchProvider>>,
    default_provider: String,
    fallback_providers: Vec<String>,
    /// WebFetch-based SERP scrape fallback. Consulted only after every
    /// configured provider has failed — typically when the operator's
    /// IP has hit a simultaneous rate-limit across paid APIs. `None`
    /// disables this last-resort branch (controlled by
    /// `SearchConfigInternal.web_fetch_fallback`, default-true).
    web_fetch_fallback: Option<Arc<WebFetchSerpFallback>>,
}

impl SearchRegistry {
    /// Create an empty registry
    pub fn new(default_provider: impl Into<String>) -> Self {
        Self {
            providers: HashMap::new(),
            default_provider: default_provider.into(),
            fallback_providers: Vec::new(),
            web_fetch_fallback: None,
        }
    }

    /// Install (or replace) the WebFetch-based SERP scrape fallback.
    ///
    /// Once installed, [`Self::search`] will attempt the fallback after
    /// the configured default + `fallback_providers` chain has fully
    /// failed. Pass `None` to disable the last-resort branch — e.g. in
    /// hermetic tests that must not make outbound HTTP under any
    /// circumstance.
    pub fn set_web_fetch_fallback(&mut self, fallback: Option<Arc<WebFetchSerpFallback>>) {
        self.web_fetch_fallback = fallback;
    }

    /// Returns true if a `WebFetch` SERP fallback is currently armed.
    /// Used by the panel / `aleph doctor` to surface the user-visible
    /// "fallback enabled" indicator without leaking the inner Arc.
    #[must_use]
    pub const fn has_web_fetch_fallback(&self) -> bool {
        self.web_fetch_fallback.is_some()
    }

    /// Build a `SearchRegistry` from `[search]` TOML configuration.
    ///
    /// Walks `config.backends` and asks the default [`crate::search::ProviderFactoryRegistry`]
    /// to construct each backend; unknown `provider_type` values and
    /// missing credentials are skipped with a warning rather than aborting
    /// the load. Returns `None` when the config is `None`, search is
    /// disabled, or no usable backend was constructed — caller should
    /// then leave `BuiltinToolConfig.search_registry = None` and the
    /// `search` tool falls back to its legacy `TAVILY_API_KEY` path.
    #[must_use]
    pub fn from_config(
        config: Option<&crate::config::types::SearchConfigInternal>,
    ) -> Option<Self> {
        Self::from_config_with_factories(
            config,
            &crate::search::ProviderFactoryRegistry::with_defaults(),
        )
    }

    /// Same as [`SearchRegistry::from_config`] but with an injectable factory
    /// table — exposed so tests can build a registry around a controlled
    /// provider set without depending on the global factory list.
    #[must_use]
    pub fn from_config_with_factories(
        config: Option<&crate::config::types::SearchConfigInternal>,
        factories: &crate::search::ProviderFactoryRegistry,
    ) -> Option<Self> {
        let cfg = config?;
        if !cfg.enabled {
            return None;
        }
        // rust-doctor-disable-next-line excessive-clone
        let mut registry = Self::new(cfg.default_provider.clone());
        let mut any_added = false;
        for (name, backend) in &cfg.backends {
            match factories.build(name, backend) {
                Ok(Some(provider)) => {
                    // rust-doctor-disable-next-line excessive-clone
                    registry.add_provider(name.clone(), provider);
                    any_added = true;
                }
                Ok(None) => {
                    // Factory chose to skip (missing credentials, unknown
                    // provider_type, etc.) — already logged at WARN by
                    // either the factory itself or `ProviderFactoryRegistry::build`.
                }
                Err(e) => {
                    log::warn!(
                        "search backend '{name}' ({}) hard-construct failed: {e}",
                        backend.provider_type
                    );
                }
            }
        }
        if !any_added {
            log::warn!(
                "[search] block parsed but no provider was constructable — \
                 search tool will fall back to TAVILY_API_KEY env var"
            );
            return None;
        }
        // Defensive (P7): the configured `default_provider` may name a backend
        // that was skipped above (missing credentials / unknown provider_type).
        // A registry whose default isn't among the constructed providers would
        // fail EVERY search with "Default provider not found" — even though
        // usable backends exist — unless the operator also happened to list
        // them under `fallback_providers`. Promote a deterministically-chosen
        // constructed provider to default so the working backends stay
        // reachable. (`any_added` guarantees `providers` is non-empty here.)
        if !registry.providers.contains_key(&registry.default_provider) {
            let mut names: Vec<&String> = registry.providers.keys().collect();
            names.sort();
            if let Some(promoted) = names.first() {
                log::warn!(
                    "[search] default_provider '{}' was not constructed (missing config?); \
                     promoting '{}' to default so usable backends remain reachable",
                    registry.default_provider,
                    promoted
                );
                // rust-doctor-disable-next-line excessive-clone
                registry.default_provider = (*promoted).clone();
            }
        }
        if let Some(ref fallbacks) = cfg.fallback_providers {
            // rust-doctor-disable-next-line excessive-clone
            registry.set_fallback_providers(fallbacks.clone());
        }
        // Wire WebFetch SERP fallback when the operator hasn't opted
        // out. The fallback only fires after the configured provider
        // chain is fully exhausted, so the worst it can do on the
        // happy path is sit in memory unused (one `reqwest::Client`).
        if cfg.web_fetch_fallback {
            match WebFetchSerpFallback::new() {
                Ok(fb) => {
                    log::info!(
                        "[search] WebFetch SERP fallback armed — DDG mirrors will be \
                         used if every configured provider fails"
                    );
                    registry.set_web_fetch_fallback(Some(Arc::new(fb)));
                }
                Err(e) => log::warn!(
                    "[search] failed to construct WebFetch SERP fallback: {e} — \
                     proceeding without last-resort scrape"
                ),
            }
        }
        Some(registry)
    }

    /// Add a provider to the registry
    pub fn add_provider(&mut self, name: String, provider: Arc<dyn SearchProvider>) {
        self.providers.insert(name, provider);
    }

    /// Set fallback providers
    pub fn set_fallback_providers(&mut self, providers: Vec<String>) {
        self.fallback_providers = providers;
    }

    /// Get a provider by name
    #[must_use]
    pub fn get_provider(&self, name: &str) -> Option<&Arc<dyn SearchProvider>> {
        self.providers.get(name)
    }

    /// Execute search with fallback logic
    ///
    /// Tries default provider first, then falls back to alternatives if it fails.
    /// Aggregates error messages from all attempted providers.
    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let mut errors: Vec<String> = Vec::new();

        // Try default provider
        if let Some(provider) = self.providers.get(&self.default_provider) {
            if !provider.is_available() {
                let msg = format!(
                    "Default provider '{}' is not available (missing configuration)",
                    self.default_provider
                );
                log::warn!("{msg}");
                errors.push(msg);
            } else {
                match provider.search(query, options).await {
                    Ok(results) => return Ok(results),
                    Err(e) => {
                        let kind = classify_search_error(&e);
                        let msg = format!(
                            "Provider '{}' [{}] failed: {}",
                            self.default_provider, kind, e
                        );
                        log::warn!(
                            target: "search",
                            "provider={} kind={} {}",
                            self.default_provider,
                            kind,
                            e
                        );
                        errors.push(msg);
                    }
                }
            }
        } else {
            let msg = format!("Default provider '{}' not found", self.default_provider);
            log::warn!("{msg}");
            errors.push(msg);
        }

        // Try fallback providers
        for provider_name in &self.fallback_providers {
            if let Some(provider) = self.providers.get(provider_name) {
                if !provider.is_available() {
                    let msg = format!(
                        "Fallback provider '{provider_name}' is not available (missing configuration)"
                    );
                    log::warn!("{msg}");
                    errors.push(msg);
                    continue;
                }
                match provider.search(query, options).await {
                    Ok(results) => {
                        log::info!("Search succeeded with fallback provider '{provider_name}'");
                        return Ok(results);
                    }
                    Err(e) => {
                        let kind = classify_search_error(&e);
                        let msg = format!("Provider '{provider_name}' [{kind}] failed: {e}");
                        log::warn!(
                            target: "search",
                            "provider={provider_name} kind={kind} {e}"
                        );
                        errors.push(msg);
                    }
                }
            }
        }

        // LAST-RESORT BRANCH (Round-2): every configured provider has
        // failed. If the operator hasn't disabled the WebFetch SERP
        // fallback, try scraping no-credential mirrors before giving
        // up. This is the difference between "search degrades during
        // rate-limit storms" and "search hard-fails until quota
        // resets" — see [`WebFetchSerpFallback`] module docs.
        if let Some(ref fallback) = self.web_fetch_fallback {
            log::info!(
                target: "search",
                "all configured providers failed; attempting WebFetch SERP fallback"
            );
            match fallback.search(query, options).await {
                Ok(results) => return Ok(results),
                Err(e) => {
                    let kind = classify_search_error(&e);
                    errors.push(format!("web-fetch-fallback [{kind}] {e}"));
                }
            }
        }

        let summary = format!("All search providers failed: {}", errors.join("; "));
        Err(AlephError::provider(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_search_error_covers_observable_kinds() {
        let cases: &[(AlephError, &'static str)] = &[
            (AlephError::authentication("brave", "bad token"), "auth"),
            (AlephError::rate_limit("429 too many"), "rate-limit"),
            (AlephError::network("dns failure"), "network"),
            (AlephError::provider("5xx upstream"), "provider"),
            (AlephError::Cancelled, "cancelled"),
        ];
        for (err, expected) in cases {
            assert_eq!(
                classify_search_error(err),
                *expected,
                "wrong kind for: {err}",
            );
        }
    }

    /// Mock provider for testing
    struct MockProvider {
        name: String,
        should_fail: bool,
        result_count: usize,
    }

    impl MockProvider {
        fn new(name: &str, should_fail: bool, result_count: usize) -> Self {
            Self {
                name: name.to_string(),
                should_fail,
                result_count,
            }
        }
    }

    #[async_trait::async_trait]
    impl SearchProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
            if self.should_fail {
                return Err(AlephError::network("Mock provider failure"));
            }

            let mut results = Vec::new();
            for i in 0..self.result_count.min(options.max_results) {
                results.push(SearchResult {
                    title: format!("{} - Result {}", query, i + 1),
                    url: format!("https://example.com/{}", i + 1),
                    snippet: format!("Snippet for result {}", i + 1),
                    full_content: None,
                    provider: Some(self.name.clone()),
                    relevance_score: Some(1.0 - (i as f32 * 0.1)),
                });
            }
            Ok(results)
        }
    }

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = SearchRegistry::new("tavily".to_string());
        assert_eq!(registry.default_provider, "tavily");
        assert!(registry.providers.is_empty());
    }

    #[tokio::test]
    async fn test_registry_add_provider() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockProvider::new("mock", false, 3);

        registry.add_provider("mock".to_string(), Arc::new(provider));

        assert!(registry.get_provider("mock").is_some());
    }

    #[tokio::test]
    async fn test_registry_search_with_mock_provider() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockProvider::new("mock", false, 5);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let options = SearchOptions {
            max_results: 3,
            timeout_seconds: 5,
            ..Default::default()
        };

        let results = registry.search("test query", &options).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "test query - Result 1");
        assert_eq!(results[0].provider, Some("mock".to_string()));
    }

    #[tokio::test]
    async fn test_registry_search_no_provider() {
        let registry = SearchRegistry::new("nonexistent".to_string());
        let options = SearchOptions::default();

        let result = registry.search("test", &options).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("All search providers failed"));
    }

    #[tokio::test]
    async fn test_registry_fallback_to_default() {
        let mut registry = SearchRegistry::new("default".to_string());

        // Add default provider that succeeds
        let default_provider = MockProvider::new("default", false, 3);
        registry.add_provider("default".to_string(), Arc::new(default_provider));

        // Set fallback to nonexistent provider
        registry.set_fallback_providers(vec!["nonexistent".to_string()]);

        let options = SearchOptions::default();
        let results = registry.search("test", &options).await.unwrap();

        // Should get results from default provider
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].provider, Some("default".to_string()));
    }

    #[tokio::test]
    async fn test_registry_fallback_chain() {
        let mut registry = SearchRegistry::new("primary".to_string());

        // Primary provider fails
        let primary = MockProvider::new("primary", true, 0);
        registry.add_provider("primary".to_string(), Arc::new(primary));

        // First fallback fails
        let fallback1 = MockProvider::new("fallback1", true, 0);
        registry.add_provider("fallback1".to_string(), Arc::new(fallback1));

        // Second fallback succeeds
        let fallback2 = MockProvider::new("fallback2", false, 2);
        registry.add_provider("fallback2".to_string(), Arc::new(fallback2));

        registry.set_fallback_providers(vec!["fallback1".to_string(), "fallback2".to_string()]);

        let options = SearchOptions::default();
        let results = registry.search("test", &options).await.unwrap();

        // Should get results from second fallback
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider, Some("fallback2".to_string()));
    }

    #[tokio::test]
    async fn test_registry_all_providers_fail() {
        let mut registry = SearchRegistry::new("primary".to_string());

        // All providers fail
        let primary = MockProvider::new("primary", true, 0);
        registry.add_provider("primary".to_string(), Arc::new(primary));

        let fallback = MockProvider::new("fallback", true, 0);
        registry.add_provider("fallback".to_string(), Arc::new(fallback));

        registry.set_fallback_providers(vec!["fallback".to_string()]);

        let options = SearchOptions::default();
        let result = registry.search("test", &options).await;

        // Should fail when all providers fail
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_registry_respects_max_results() {
        let mut registry = SearchRegistry::new("mock".to_string());

        // Provider can return 10 results
        let provider = MockProvider::new("mock", false, 10);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let options = SearchOptions {
            max_results: 5,
            timeout_seconds: 5,
            ..Default::default()
        };

        let results = registry.search("test", &options).await.unwrap();

        // Should only get max_results
        assert_eq!(results.len(), 5);
    }

    // ─── WebFetch SERP fallback wiring (Round-2) ────────────────────────

    #[tokio::test]
    async fn fallback_not_consulted_when_primary_succeeds() {
        // Primary succeeds → fallback must NOT be entered. We assert
        // this by installing a fallback whose mirrors are all pre-cooled
        // (so any call would deterministically fail), and confirming
        // search() still returns success.
        let mut registry = SearchRegistry::new("primary".to_string());
        registry.add_provider(
            "primary".to_string(),
            Arc::new(MockProvider::new("primary", false, 3)),
        );

        let fb = WebFetchSerpFallback::new().expect("construct");
        // Pre-cool every mirror — any actual call would error.
        fb.force_cooldown_for("ddg-lite");
        fb.force_cooldown_for("ddg-html");
        registry.set_web_fetch_fallback(Some(Arc::new(fb)));
        assert!(registry.has_web_fetch_fallback());

        let results = registry
            .search("rust", &SearchOptions::default())
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            3,
            "primary results should be returned without touching fallback"
        );
        assert_eq!(results[0].provider, Some("primary".to_string()));
    }

    #[tokio::test]
    async fn fallback_error_aggregated_when_all_providers_and_fallback_fail() {
        // Every provider fails AND fallback mirrors are pre-cooled —
        // the final aggregate error must include both the provider
        // failures AND the `web-fetch-fallback` line so operators
        // know the last-resort branch was actually attempted.
        let mut registry = SearchRegistry::new("primary".to_string());
        registry.add_provider(
            "primary".to_string(),
            Arc::new(MockProvider::new("primary", true, 0)),
        );
        registry.add_provider(
            "fallback1".to_string(),
            Arc::new(MockProvider::new("fallback1", true, 0)),
        );
        registry.set_fallback_providers(vec!["fallback1".to_string()]);

        let fb = WebFetchSerpFallback::new().unwrap();
        fb.force_cooldown_for("ddg-lite");
        fb.force_cooldown_for("ddg-html");
        registry.set_web_fetch_fallback(Some(Arc::new(fb)));

        let err = registry
            .search("x", &SearchOptions::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("All search providers failed"),
            "missing provider failure prefix: {err}"
        );
        assert!(
            err.contains("web-fetch-fallback"),
            "fallback attempt missing from aggregate: {err}"
        );
        assert!(
            err.contains("ddg-lite") || err.contains("ddg-html"),
            "fallback mirror name missing: {err}"
        );
    }

    #[test]
    fn set_web_fetch_fallback_toggles_arm_flag() {
        let mut registry = SearchRegistry::new("mock".to_string());
        assert!(!registry.has_web_fetch_fallback());
        registry.set_web_fetch_fallback(Some(Arc::new(WebFetchSerpFallback::new().unwrap())));
        assert!(registry.has_web_fetch_fallback());
        registry.set_web_fetch_fallback(None);
        assert!(!registry.has_web_fetch_fallback());
    }

    #[test]
    fn from_config_arms_fallback_by_default() {
        use crate::config::types::{SearchBackendConfig, SearchConfigInternal};
        // A config with no api keys would yield no providers and
        // from_config returns None — make sure we get a registry by
        // including DDG (zero-cred provider) so `any_added` is true.
        let mut backends = HashMap::new();
        backends.insert(
            "ddg".to_string(),
            SearchBackendConfig {
                provider_type: "duckduckgo".to_string(),
                api_key: None,
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        let cfg = SearchConfigInternal {
            enabled: true,
            default_provider: "ddg".to_string(),
            fallback_providers: None,
            max_results: 5,
            timeout_seconds: 10,
            backends,
            web_fetch_fallback: true,
        };
        let registry = SearchRegistry::from_config(Some(&cfg)).expect("registry");
        assert!(
            registry.has_web_fetch_fallback(),
            "default web_fetch_fallback=true must arm the fallback"
        );
    }

    #[test]
    fn from_config_promotes_default_when_configured_default_unconstructable() {
        use crate::config::types::{SearchBackendConfig, SearchConfigInternal};
        // default_provider names "tavily" but tavily has no api_key, so it is
        // skipped during construction. The zero-cred "ddg" backend DOES
        // construct. Without promotion the registry's default would point at
        // the absent "tavily" and every search would hard-fail.
        let mut backends = HashMap::new();
        backends.insert(
            "tavily".to_string(),
            SearchBackendConfig {
                provider_type: "tavily".to_string(),
                api_key: None,
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        backends.insert(
            "ddg".to_string(),
            SearchBackendConfig {
                provider_type: "duckduckgo".to_string(),
                api_key: None,
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        let cfg = SearchConfigInternal {
            enabled: true,
            default_provider: "tavily".to_string(),
            fallback_providers: None,
            max_results: 5,
            timeout_seconds: 10,
            backends,
            web_fetch_fallback: false,
        };
        let registry = SearchRegistry::from_config(Some(&cfg)).expect("registry");
        assert_eq!(
            registry.default_provider, "ddg",
            "unconstructable default must be promoted to a constructed provider"
        );
        assert!(registry.get_provider("ddg").is_some());
    }

    #[test]
    fn from_config_respects_opt_out() {
        use crate::config::types::{SearchBackendConfig, SearchConfigInternal};
        let mut backends = HashMap::new();
        backends.insert(
            "ddg".to_string(),
            SearchBackendConfig {
                provider_type: "duckduckgo".to_string(),
                api_key: None,
                base_url: None,
                engine_id: None,
                engines: None,
                min_request_interval_ms: None,
                verified: false,
            },
        );
        let cfg = SearchConfigInternal {
            enabled: true,
            default_provider: "ddg".to_string(),
            fallback_providers: None,
            max_results: 5,
            timeout_seconds: 10,
            backends,
            web_fetch_fallback: false,
        };
        let registry = SearchRegistry::from_config(Some(&cfg)).expect("registry");
        assert!(
            !registry.has_web_fetch_fallback(),
            "web_fetch_fallback=false must leave the registry without a fallback"
        );
    }
}

use crate::error::{AlephError, Result};
use crate::search::notes::{all_empty, answered_after_failures, degraded};
use crate::search::{
    SearchCapabilities, SearchOptions, SearchProvider, SearchResult, WebFetchSerpFallback,
};
use crate::sync_primitives::Arc;

/// The name the SERP scrape fallback answers under.
///
/// It is not a registered provider, so it has no `name()` to ask; the string
/// reaches an operator's log line and a caller's `SearchAnswer.provider`, and
/// two spellings of it would be two different backends to anyone grepping.
const WEB_FETCH_FALLBACK_NAME: &str = "web-fetch-fallback";

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

/// What a search returned, together with what it could not do.
///
/// The results alone cannot carry the second half: a dimension the answering
/// backend has no parameter for produces a perfectly ordinary-looking result
/// set. Only the registry knows which backend answered and what that backend
/// declared, so the notes are assembled here and travel with the results
/// rather than being re-derived by a layer that cannot see either.
#[derive(Debug, Clone)]
pub struct SearchAnswer {
    /// The results, in the answering backend's order.
    pub results: Vec<SearchResult>,
    /// Which backend answered. Not always the one the chain starts with.
    pub provider: String,
    /// Sentences from [`crate::search::notes`] naming what this answer is
    /// missing and which lever the caller can pull. Empty is the common case.
    pub notes: Vec<String>,
}

/// Registry for managing multiple search providers
///
/// Maintains a pool of configured providers and routes search requests
/// to the appropriate backend based on configuration.
pub struct SearchRegistry {
    providers: HashMap<String, Arc<dyn SearchProvider>>,
    default_provider: String,
    fallback_providers: Vec<String>,
    /// Operator-configured search defaults (`[search].max_results` /
    /// `[search].timeout_seconds`), as the [`SearchOptions`] a caller should
    /// start from. `None` for a hand-built registry, which keeps
    /// [`SearchOptions::default`].
    ///
    /// Lives here because [`Self::from_config`] is the only place the `[search]`
    /// block is read; without it the two knobs were validated, editable in the
    /// Panel and persisted, yet nothing ever read them — the live caller
    /// (`SearchTool::call_impl`) built `SearchOptions::default()` with its
    /// hardcoded 5 / 10s.
    config_defaults: Option<SearchOptions>,
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
            config_defaults: None,
            web_fetch_fallback: None,
        }
    }

    /// The [`SearchOptions`] a caller should start from: the operator's
    /// `[search]` defaults when this registry was built from config, otherwise
    /// [`SearchOptions::default`].
    #[must_use]
    pub fn default_options(&self) -> SearchOptions {
        self.config_defaults.clone().unwrap_or_default()
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
        registry.config_defaults = Some(SearchOptions {
            max_results: cfg.max_results,
            timeout_seconds: cfg.timeout_seconds,
            ..SearchOptions::default()
        });
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

    /// Which dimensions this request actually asks for.
    fn requested(options: &SearchOptions) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: !options.include_domains.is_empty()
                || !options.exclude_domains.is_empty(),
            recency: options.recency.is_some(),
            full_content: options.include_full_content,
        }
    }

    /// The notes owed to a caller who asked for a dimension the answering
    /// backend cannot express.
    ///
    /// Derived from the same two values [`Self::ordered_candidates`] sorts on
    /// — what the request asks for, and what the backend declares — so the
    /// backend that was picked *despite* not carrying a dimension is exactly
    /// the one that says so. Phrasing the comparison a second way here is how
    /// the sort and the note drift apart.
    fn degradation_notes(
        options: &SearchOptions,
        provider: &str,
        have: SearchCapabilities,
    ) -> Vec<String> {
        let want = Self::requested(options);
        // The dimension is named as the *caller* spelled it, not as the field
        // is spelled here: a note naming a lever the reader cannot find in
        // their own request is an apology, not a note. Both domain lists share
        // one capability bit, so which word to use comes from what was set.
        let domains = match (
            options.include_domains.is_empty(),
            options.exclude_domains.is_empty(),
        ) {
            (true, false) => "exclude_domains",
            (false, false) => "domains/exclude_domains",
            _ => "domains",
        };
        [
            (domains, want.domain_filter, have.domain_filter),
            ("recency", want.recency, have.recency),
            ("full_content", want.full_content, have.full_content),
        ]
        .into_iter()
        .filter(|&(_, want, have)| want && !have)
        .map(|(dimension, _, _)| degraded(dimension, provider))
        .collect()
    }

    /// Assemble the answer a backend just produced, with the notes it owes.
    ///
    /// One place, because the three points that can produce an answer (a named
    /// backend, the chain, the SERP fallback) owe the same three sentences for
    /// the same three reasons; written out at each of them they come out
    /// nearly-but-not-quite the same. `answered_empty` is how many backends
    /// were asked and came back with nothing — it is only read when `results`
    /// is empty, which is the only case where that count is an answer.
    fn answer(
        options: &SearchOptions,
        provider: String,
        have: SearchCapabilities,
        results: Vec<SearchResult>,
        failed: usize,
        answered_empty: usize,
    ) -> SearchAnswer {
        let mut notes = Self::degradation_notes(options, &provider, have);
        if failed > 0 {
            notes.push(answered_after_failures(&provider, failed));
        }
        if results.is_empty() {
            notes.push(all_empty(answered_empty));
        }
        SearchAnswer {
            results,
            provider,
            notes,
        }
    }

    /// Default first, then fallbacks in configuration order, then stably
    /// reordered so providers that can carry every requested dimension come
    /// first.
    ///
    /// Stable on purpose: within a group the configured order survives, so the
    /// same query reaches the same backend on every call. An unstable sort
    /// would trade that for nothing anyone asked for.
    fn ordered_candidates(&self, options: &SearchOptions) -> Vec<String> {
        let want = Self::requested(options);
        // An order-preserving seen-set, not `Vec::dedup` — `dedup` only
        // catches *consecutive* repeats, and a provider named as both the
        // default and again in `fallback_providers` produces a duplicate
        // that is never adjacent (`[default, ...fallbacks]`). Missing that
        // would consult the same backend twice, spend its quota twice, and
        // count its failure twice against the chain.
        let mut seen = std::collections::HashSet::new();
        let mut names: Vec<String> = std::iter::once(self.default_provider.clone())
            .chain(self.fallback_providers.iter().cloned())
            .filter(|n| self.providers.contains_key(n))
            .filter(|n| seen.insert(n.clone()))
            .collect();
        names.sort_by_key(|n| {
            let have = self.providers[n].capabilities();
            let satisfies = (!want.domain_filter || have.domain_filter)
                && (!want.recency || have.recency)
                && (!want.full_content || have.full_content);
            usize::from(!satisfies)
        });
        names
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

    /// Execute search, honouring an explicit provider or falling back through
    /// the configured chain.
    ///
    /// When `options.provider` names a backend, only that backend is
    /// consulted: an unknown or unavailable name is a hard failure, never a
    /// silent fallback to a backend the caller did not choose. Otherwise the
    /// candidates are the default provider, then `fallback_providers`,
    /// stably reordered so a backend that can carry every dimension this
    /// request asks for goes first. A backend answering with zero results
    /// does not end the chain — the rest, then the `WebFetch` SERP fallback,
    /// still get a turn. Only a chain where nobody answered (every attempt
    /// errored, or there were no candidates at all) is an `Err`, reported as
    /// a structured `name [kind] message` line per attempted backend.
    ///
    /// Whatever the answer, it carries the notes it owes: a dimension the
    /// answering backend could not express, the failures it answered after,
    /// and an empty result set said out loud as an answer rather than left to
    /// read as a failure.
    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<SearchAnswer> {
        // Naming a provider is an instruction, not a preference: resolve and
        // delegate to it alone, without touching the fallback chain below.
        if let Some(name) = &options.provider {
            let Some(p) = self.providers.get(name).filter(|p| p.is_available()) else {
                let mut known: Vec<&str> = self.providers.keys().map(String::as_str).collect();
                known.sort_unstable();
                return Err(AlephError::invalid_config(format!(
                    "search provider '{name}' is not configured or not available; \
                     configured: {}",
                    known.join(", ")
                )));
            };
            let results = p.search(query, options).await?;
            // A named backend is the whole chain: nothing failed ahead of it,
            // and its empty answer is still an answer from the one backend
            // that was asked.
            return Ok(Self::answer(
                options,
                name.clone(),
                p.capabilities(),
                results,
                0,
                1,
            ));
        }

        let mut errors: Vec<String> = Vec::new();
        // Providers that answered with zero results. Tracked separately from
        // `errors` so the end of this function can tell "nobody found it"
        // (worth reporting as an empty `Ok`) from "nobody was asked at all"
        // (only errors, or no candidates — still a hard failure).
        let mut empty: Vec<String> = Vec::new();

        // Ordered candidates: default first, then fallbacks, stably
        // reordered so a provider that can carry every dimension this
        // request asks for is tried before one that cannot. `ordered_candidates`
        // already dropped anything not in `self.providers`, so the lookup
        // below cannot miss.
        for provider_name in self.ordered_candidates(options) {
            let provider = &self.providers[&provider_name];
            if !provider.is_available() {
                let msg = format!("{provider_name} [unavailable] missing configuration");
                log::warn!("{msg}");
                errors.push(msg);
                continue;
            }
            match provider.search(query, options).await {
                Ok(results) if !results.is_empty() => {
                    if provider_name != self.default_provider {
                        log::info!("Search succeeded with fallback provider '{provider_name}'");
                    }
                    let have = provider.capabilities();
                    return Ok(Self::answer(
                        options,
                        provider_name,
                        have,
                        results,
                        errors.len(),
                        0,
                    ));
                }
                Ok(_) => {
                    // Answering "nothing" is not a reason to stop asking: a
                    // fallback list exists precisely because backends
                    // disagree about what exists. Keep trying the rest of
                    // the chain (and the SERP fallback below) instead of
                    // treating an empty answer as the final word.
                    empty.push(provider_name);
                }
                Err(e) => {
                    let kind = classify_search_error(&e);
                    let msg = format!("{provider_name} [{kind}] {e}");
                    log::warn!(
                        target: "search",
                        "provider={provider_name} kind={kind} {e}"
                    );
                    errors.push(msg);
                }
            }
        }

        // LAST-RESORT BRANCH (Round-2): no provider returned a non-empty
        // result. If the operator hasn't disabled the WebFetch SERP
        // fallback, try scraping no-credential mirrors before giving
        // up — "every backend came back empty" is at least as often
        // blocked egress or an expired credential that still answers
        // 200 as it is a true zero, which is exactly the situation this
        // fallback exists for. See [`WebFetchSerpFallback`] module docs.
        if let Some(ref fallback) = self.web_fetch_fallback {
            log::info!(
                target: "search",
                "no provider returned results; attempting WebFetch SERP fallback"
            );
            match fallback.search(query, options).await {
                Ok(results) => {
                    // The scraper forwards only the timeout and the result
                    // count (see [`WebFetchSerpFallback::search`]), so it
                    // carries none of the dimensions a caller can ask for.
                    return Ok(Self::answer(
                        options,
                        WEB_FETCH_FALLBACK_NAME.to_string(),
                        SearchCapabilities::default(),
                        results,
                        errors.len(),
                        empty.len() + 1,
                    ));
                }
                Err(e) => {
                    let kind = classify_search_error(&e);
                    errors.push(format!("{WEB_FETCH_FALLBACK_NAME} [{kind}] {e}"));
                }
            }
        }

        // Decided once, here, after the fallback has already had its turn:
        // if at least one backend answered — with nothing, but an answer —
        // that is a legitimate empty result, not a failure. Reporting
        // failure for a question that was answered would tell the model to
        // retry something it already has the answer to. Only a chain where
        // *nobody* answered (every attempt errored, or there were no
        // candidates at all) is an `Err`.
        match empty.first() {
            None => {
                // One `name [kind] message` line per attempted backend, headed
                // by a summary line — the classifier's `kind` has computed a
                // label for every failure since it was written; this report is
                // its first real consumer, read by both the model deciding
                // whether to retry and the operator grepping the log.
                let mut lines = vec!["All search providers failed:".to_string()];
                lines.extend(errors);
                Err(AlephError::provider(lines.join("\n")))
            }
            Some(first) => {
                // Somebody answered — with nothing. `empty` is in chain order,
                // so the first entry is the backend whose answer this is.
                let have = self.providers[first].capabilities();
                Ok(Self::answer(
                    options,
                    first.clone(),
                    have,
                    Vec::new(),
                    errors.len(),
                    empty.len(),
                ))
            }
        }
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
        capabilities: SearchCapabilities,
    }

    impl MockProvider {
        fn new(name: &str, should_fail: bool, result_count: usize) -> Self {
            Self {
                name: name.to_string(),
                should_fail,
                result_count,
                capabilities: SearchCapabilities::default(),
            }
        }

        /// Declares `domain_filter` support, for capability-ordering tests.
        fn with_domain_filter(mut self) -> Self {
            self.capabilities.domain_filter = true;
            self
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

        fn capabilities(&self) -> SearchCapabilities {
            self.capabilities
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
                    published_date: None,
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

        let results = registry
            .search("test query", &options)
            .await
            .unwrap()
            .results;

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
        let results = registry.search("test", &options).await.unwrap().results;

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
        let answer = registry.search("test", &options).await.unwrap();

        // Should get results from second fallback
        assert_eq!(answer.results.len(), 2);
        assert_eq!(answer.results[0].provider, Some("fallback2".to_string()));
        assert_eq!(
            answer.provider, "fallback2",
            "the answer names who answered"
        );
        assert!(
            answer.notes.iter().any(|n| n.contains("fallback2")),
            "two backends failed before it answered: {:?}",
            answer.notes
        );
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

        let results = registry.search("test", &options).await.unwrap().results;

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
            .unwrap()
            .results;
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
    fn from_config_carries_operator_defaults_into_default_options() {
        use crate::config::types::{SearchBackendConfig, SearchConfigInternal};
        // `[search].max_results` / `.timeout_seconds` were validated, editable in
        // the Panel and persisted, but nothing read them at runtime: the live
        // caller built `SearchOptions::default()` (5 / 10s). Pin the wire.
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
            max_results: 17,
            timeout_seconds: 42,
            backends,
            web_fetch_fallback: false,
        };
        let registry = SearchRegistry::from_config(Some(&cfg)).expect("registry");
        let options = registry.default_options();
        assert_eq!(options.max_results, 17);
        assert_eq!(options.timeout_seconds, 42);

        // A hand-built registry keeps the type's own defaults.
        assert_eq!(
            SearchRegistry::new("mock").default_options().max_results,
            SearchOptions::default().max_results
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

    // ─── Capability-aware ordering (Round-4) ────────────────────────────

    #[tokio::test]
    async fn a_provider_that_can_carry_the_requested_dimension_goes_first() {
        let mut reg = SearchRegistry::new("plain");
        reg.add_provider(
            "plain".into(),
            Arc::new(MockProvider::new("plain", false, 1)),
        );
        reg.add_provider(
            "rich".into(),
            Arc::new(MockProvider::new("rich", false, 1).with_domain_filter()),
        );
        reg.set_fallback_providers(vec!["rich".into()]);

        let opts = SearchOptions {
            include_domains: vec!["github.com".into()],
            ..Default::default()
        };
        assert_eq!(reg.ordered_candidates(&opts), vec!["rich", "plain"]);

        // No dimension requested => configuration order is untouched.
        assert_eq!(
            reg.ordered_candidates(&SearchOptions::default()),
            vec!["plain", "rich"]
        );
    }

    /// Stable within a group: two providers that both satisfy (or both fail to
    /// satisfy) the request keep configuration order, so the same query lands on
    /// the same backend every time. Non-determinism here would make a cached or
    /// rate-limited backend impossible to reason about.
    #[tokio::test]
    async fn ordering_is_stable_within_a_capability_group() {
        let mut reg = SearchRegistry::new("a");
        for n in ["a", "b", "c"] {
            reg.add_provider(n.into(), Arc::new(MockProvider::new(n, false, 1)));
        }
        reg.set_fallback_providers(vec!["b".into(), "c".into()]);
        for _ in 0..20 {
            assert_eq!(
                reg.ordered_candidates(&SearchOptions::default()),
                vec!["a", "b", "c"]
            );
        }
    }

    /// A provider named both as the default and again in the fallback list is one
    /// backend, not two. `Vec::dedup` would not catch this — the duplicates are not
    /// adjacent — so the search would consult the same backend twice, spend twice, and
    /// count its failure twice against the chain.
    #[tokio::test]
    async fn a_provider_named_twice_is_consulted_once() {
        let mut reg = SearchRegistry::new("a");
        for n in ["a", "b"] {
            reg.add_provider(n.into(), Arc::new(MockProvider::new(n, false, 1)));
        }
        reg.set_fallback_providers(vec!["b".into(), "a".into()]);
        assert_eq!(
            reg.ordered_candidates(&SearchOptions::default()),
            vec!["a", "b"]
        );
    }

    // ─── Empty results continue the chain (Task 5) ──────────────────────

    /// A backend answering "zero results" is answering, but it is not an answer
    /// worth ending the chain on: the whole point of a fallback list is that the
    /// backends disagree about what exists. Before this, a default provider that
    /// returned an empty list stopped eight others and the SERP fallback from
    /// ever being asked.
    #[tokio::test]
    async fn an_empty_result_set_does_not_end_the_chain() {
        let mut reg = SearchRegistry::new("empty");
        reg.add_provider(
            "empty".into(),
            Arc::new(MockProvider::new("empty", false, 0)),
        );
        reg.add_provider("full".into(), Arc::new(MockProvider::new("full", false, 3)));
        reg.set_fallback_providers(vec!["full".into()]);

        let out = reg.search("q", &SearchOptions::default()).await.unwrap();
        assert_eq!(
            out.results.len(),
            3,
            "the chain must continue past a zero-result answer"
        );
    }

    /// All empty is still a legitimate answer — an empty Ok, never an Err.
    /// Folding "nobody found anything" into an error would make the model retry
    /// a question that was answered.
    #[tokio::test]
    async fn all_backends_empty_returns_an_empty_ok() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 0)));
        reg.add_provider("b".into(), Arc::new(MockProvider::new("b", false, 0)));
        reg.set_fallback_providers(vec!["b".into()]);
        let out = reg.search("q", &SearchOptions::default()).await.unwrap();
        assert!(out.results.is_empty());
        assert!(
            out.notes.iter().any(|n| n.contains('2')),
            "both backends were asked and both answered nothing: {:?}",
            out.notes
        );
    }

    /// A backend that answered "nothing" answered. Only a chain where *nobody*
    /// answered is a failure — folding the two together tells the model to retry a
    /// question that already has an answer.
    #[tokio::test]
    async fn an_error_plus_an_empty_answer_is_still_an_answer() {
        let mut reg = SearchRegistry::new("boom");
        reg.add_provider("boom".into(), Arc::new(MockProvider::new("boom", true, 0)));
        reg.add_provider(
            "quiet".into(),
            Arc::new(MockProvider::new("quiet", false, 0)),
        );
        reg.set_fallback_providers(vec!["quiet".into()]);
        let out = reg.search("q", &SearchOptions::default()).await;
        assert!(
            out.is_ok(),
            "one backend errored and one answered with zero results; the chain answered"
        );
        assert!(out.unwrap().results.is_empty());
    }

    // ─── Explicit provider override (Task 6) ─────────────────────────────

    /// Naming a provider is an instruction, not a preference. Falling back would
    /// hand the caller results from a backend it did not choose while reporting
    /// success — a confident wrong answer, which is the expensive kind.
    #[tokio::test]
    async fn an_unknown_named_provider_fails_loudly_instead_of_falling_back() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 3)));
        let opts = SearchOptions {
            provider: Some("nope".into()),
            ..Default::default()
        };
        let err = reg.search("q", &opts).await.unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(
            err.contains('a'),
            "the error must list what IS configured: {err}"
        );
    }

    #[tokio::test]
    async fn a_named_provider_is_the_only_one_consulted() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0))); // fails
        reg.add_provider("b".into(), Arc::new(MockProvider::new("b", false, 3)));
        reg.set_fallback_providers(vec!["b".into()]);
        let opts = SearchOptions {
            provider: Some("a".into()),
            ..Default::default()
        };
        assert!(
            reg.search("q", &opts).await.is_err(),
            "must not silently use b"
        );
    }

    /// The classifier already computed a failure kind for every provider; before
    /// this it fed one log line and nothing else. The message a model and an
    /// operator both read is the right consumer.
    #[tokio::test]
    async fn the_failure_report_names_each_provider_and_its_failure_kind() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0)));
        let err = reg
            .search("q", &SearchOptions::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("a ["),
            "expected `name [kind]` framing, got: {err}"
        );
    }

    /// Dropping a dimension the caller asked for is the failure this note
    /// exists to prevent: the search still runs, but silently unfiltered on
    /// that axis reads exactly like a filtered answer.
    #[tokio::test]
    async fn a_backend_that_cannot_express_the_dimension_says_so() {
        let mut reg = SearchRegistry::new("plain");
        reg.add_provider(
            "plain".into(),
            Arc::new(MockProvider::new("plain", false, 2)),
        );
        let opts = SearchOptions {
            include_domains: vec!["github.com".into()],
            ..Default::default()
        };
        let answer = reg.search("q", &opts).await.unwrap();
        assert_eq!(answer.results.len(), 2, "the search still runs");
        assert!(
            answer
                .notes
                .iter()
                .any(|n| n.contains("domains") && n.contains("plain")),
            "silently dropping the dimension is the failure this note exists to prevent: {:?}",
            answer.notes
        );
    }
}

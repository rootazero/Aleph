use crate::error::{AlephError, Result};
use crate::search::health::ProviderHealth;
use crate::search::notes::{
    all_empty, answered_after_failures, degraded, fanout_partial, merged_duplicates,
};
use crate::search::{
    SearchCapabilities, SearchOptions, SearchProvider, SearchResult, WebFetchSerpFallback,
};
use crate::sync_primitives::Arc;

/// The default-provider name an unconfigured registry carries.
///
/// It names no backend, and that is the point: `search` on a machine with
/// nothing configured must fail with "no search backend is configured", not
/// with a complaint about a specific backend the operator never chose.
const UNCONFIGURED_DEFAULT: &str = "none";

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
    /// Which backends failed recently. Read only by [`Self::ordered_candidates`],
    /// as a sort key within a capability group — never as permission to skip
    /// one. See [`crate::search::health`] for why that distinction is the
    /// whole design.
    health: ProviderHealth,
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
            health: ProviderHealth::new(),
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

    /// Build a one-backend registry from a bare `TAVILY_API_KEY`.
    ///
    /// This replaces a second implementation of "how do I search" that lived
    /// in `SearchTool` and read the environment itself. That path predated
    /// `SearchOptions` and ignored all of it, so every parameter the tool face
    /// accepts would have been accepted, reported as applied, and dropped on
    /// any machine without a `[search]` block — which is the zero-config
    /// install, not a corner case.
    ///
    /// No SERP fallback is armed: `[search].web_fetch_fallback` is the switch
    /// for that, and an install with no `[search]` block has not touched it.
    #[must_use]
    pub fn from_env_only(api_key: &str) -> Option<Self> {
        if api_key.trim().is_empty() {
            return None;
        }
        let provider = match crate::search::providers::TavilyProvider::new(api_key.to_string()) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("TAVILY_API_KEY is set but unusable, so no backend was built: {e}");
                return None;
            }
        };
        let name = crate::search::providers::tavily::NAME;
        let mut registry = Self::new(name);
        registry.add_provider(name.to_string(), Arc::new(provider));
        Some(registry)
    }

    /// The registry a `SearchTool` is built on, given what boot constructed.
    ///
    /// Two construction points need this decision, and they used to state it
    /// twice — one of them under a comment reading "must mirror
    /// `builder/constructor/mod.rs:48`", which is a fact with two authors and
    /// no compiler between them. Order: the registry built from `[search]`,
    /// else one synthesised from a bare key, else an empty one.
    ///
    /// Never `None`. A machine with nothing configured still registers the
    /// tool and fails, when called, with a message naming what to set: a
    /// missing tool tells the model this harness cannot search, which is a
    /// different claim and a false one.
    #[must_use]
    pub fn for_tool(configured: Option<&Arc<Self>>, api_key: Option<&str>) -> Arc<Self> {
        if let Some(registry) = configured {
            return Arc::clone(registry);
        }
        api_key
            .and_then(Self::from_env_only)
            .map_or_else(|| Arc::new(Self::new(UNCONFIGURED_DEFAULT)), Arc::new)
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
        let mut defaults = SearchOptions {
            max_results: cfg.max_results,
            timeout_seconds: cfg.timeout_seconds,
            ..SearchOptions::default()
        };
        // Thread the optional `[search]` fields through to `SearchOptions`.
        // Each `cfg.* -> defaults.*` mapping is documented at the field site
        // in `SearchConfigInternal`. If a future field is added there without
        // a corresponding line here, the `dropped_keys` audit below logs the
        // omission at boot so the contract drift is visible without a test
        // having to break first.
        defaults.language = cfg.language.clone().or(defaults.language);
        defaults.region = cfg.region.clone().or(defaults.region);
        if let Some(safe) = cfg.safe_search {
            defaults.safe_search = safe;
        }
        if let Some(include) = cfg.include_domains.clone() {
            if !include.is_empty() {
                defaults.include_domains = include;
            }
        }
        if let Some(exclude) = cfg.exclude_domains.clone() {
            if !exclude.is_empty() {
                defaults.exclude_domains = exclude;
            }
        }
        registry.config_defaults = Some(defaults);
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

    /// Re-tag a backend's results with the name it is **configured** under.
    ///
    /// Each provider stamps `SearchResult::provider` with its own `NAME`,
    /// which is the provider *type* (`"searxng"`). The registry addresses
    /// backends by their configuration key (`[search.backends.alpha]`), and so
    /// does everything a caller can act on: `providers` on the tool face, the
    /// `provider_used` summary, every note. On the overwhelmingly common
    /// config where the key equals the type the two coincide, which is why
    /// this went unnoticed until a fan-out over two SearXNG instances
    /// answered `provider_used: "alpha+bravo"` with every row saying
    /// `provider: "searxng"` — two vocabularies for one fact, and the row
    /// attribution useless in the one situation it exists for.
    ///
    /// The registry's name wins because it is the addressable one. The type
    /// is still in the operator's config; the instance is not recoverable
    /// from anything else in the answer.
    ///
    /// Not applied to the SERP fallback: it tags results `fallback:<mirror>`,
    /// which is strictly more than its registry name would say, and its
    /// mirrors are not configured backends anyone can name.
    fn attributed(name: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
        results
            .into_iter()
            .map(|mut r| {
                r.provider = Some(name.to_string());
                r
            })
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
    /// first, and within that group so a backend that failed recently is
    /// tried after one that did not.
    ///
    /// Stable on purpose: within a group the configured order survives, so the
    /// same query reaches the same backend on every call. An unstable sort
    /// would trade that for nothing anyone asked for.
    ///
    /// **Capability outranks health, and that ranking is load-bearing.** The
    /// other way round, one failure from the only backend that declares a
    /// dimension hands the query to a backend that cannot carry it, which
    /// answers and says so — a note that is true (`searxng` really has no
    /// domain parameter) while the actual reason (the capable backend was
    /// demoted) appears nowhere. That is a silently worse answer wearing a
    /// truthful explanation. Health may only break ties between backends that
    /// are equally able to answer this request.
    ///
    /// Nothing here is skipped: see [`crate::search::health`] for why a
    /// demotion and a gate are different designs.
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
        // Read health once per name rather than once per comparison, which
        // also gives the log line below something to say without a second
        // pass over the map.
        let degraded: std::collections::HashSet<String> = names
            .iter()
            .filter(|n| self.health.is_degraded(n))
            .cloned()
            .collect();
        names.sort_by_key(|n| {
            let have = self.providers[n].capabilities(options);
            let satisfies = (!want.domain_filter || have.domain_filter)
                && (!want.recency || have.recency)
                && (!want.full_content || have.full_content);
            (usize::from(!satisfies), usize::from(degraded.contains(n)))
        });
        if !degraded.is_empty() {
            // The configured order is no longer literally the order tried, so
            // "why did the second backend answer?" needs one more fact than
            // the config file to answer.
            let mut names: Vec<&str> = degraded.iter().map(String::as_str).collect();
            names.sort_unstable();
            log::info!(
                target: "search",
                "demoted after a recent failure (still asked if those ahead do not answer): {}",
                names.join(", ")
            );
        }
        names
    }

    /// Resolve the backends a caller named, or say which names are not there.
    ///
    /// One rule for one name and for five: a name that resolves to nothing is
    /// a configuration mistake the caller can fix, and answering it from some
    /// other backend would be answering a question nobody asked. The error
    /// lists *all* the unresolved names rather than the first, because a
    /// caller fixing a typo should not have to re-run to discover the second
    /// one.
    fn resolve_named(&self, names: &[String]) -> Result<Vec<(String, &Arc<dyn SearchProvider>)>> {
        let mut resolved = Vec::with_capacity(names.len());
        let mut missing: Vec<&str> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                continue; // naming a backend twice asks it once
            }
            match self.providers.get(name).filter(|p| p.is_available()) {
                Some(p) => resolved.push((name.clone(), p)),
                None => missing.push(name.as_str()),
            }
        }
        if !missing.is_empty() {
            let mut known: Vec<&str> = self.providers.keys().map(String::as_str).collect();
            known.sort_unstable();
            return Err(AlephError::invalid_config(format!(
                "search provider(s) {} not configured or not available; configured: {}",
                missing
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", "),
                known.join(", ")
            )));
        }
        Ok(resolved)
    }

    /// Ask every named backend at once and merge what comes back.
    ///
    /// Concurrent because the alternative is paying each backend's latency in
    /// series for an answer that is only interesting as a whole; `join_all`
    /// rather than spawned tasks because these futures borrow the registry
    /// and the options, and nothing here outlives the call.
    ///
    /// Partial success is success: backends disagree about what exists, which
    /// is the reason to ask several, so one of them failing narrows the
    /// answer instead of ending it. Only *nobody* answering is an `Err`, and
    /// it carries the same `name [kind] message` report the chain produces.
    async fn fan_out(&self, query: &str, options: &SearchOptions) -> Result<SearchAnswer> {
        let named = self.resolve_named(&options.providers)?;
        let asked = named.len();

        let outcomes = futures::future::join_all(
            named
                .iter()
                .map(|(name, p)| async move { (name.clone(), p.search(query, options).await) }),
        )
        .await;

        let mut answered: Vec<String> = Vec::new();
        let mut per_backend: Vec<Vec<SearchResult>> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (name, outcome) in outcomes {
            match outcome {
                Ok(results) => {
                    per_backend.push(Self::attributed(&name, results));
                    answered.push(name);
                }
                Err(e) => {
                    let kind = classify_search_error(&e);
                    // Named backends are an instruction, so health never
                    // reorders or skips them — but a failure is a failure
                    // whichever face observed it, and the chain would
                    // otherwise never learn about a backend that is only ever
                    // reached by name.
                    self.health.note_failure(&name, &e);
                    log::warn!(target: "search", "provider={name} kind={kind} {e}");
                    errors.push(format!("{name} [{kind}] {e}"));
                }
            }
        }

        if answered.is_empty() {
            let mut lines = vec!["All named search providers failed:".to_string()];
            lines.extend(errors);
            return Err(AlephError::provider(lines.join("\n")));
        }

        let (results, duplicates) =
            crate::search::merge::merge_by_rank(per_backend, options.validated_max_results());

        // Degradation is per backend, not per answer: with several of them a
        // dimension can be honoured by one and dropped by another, and the
        // merged set is then filtered on that axis only in part. Each backend
        // that could not carry a requested dimension says so under its own
        // name, using the same sentence the chain uses.
        let mut notes: Vec<String> = Vec::new();
        for name in &answered {
            notes.extend(Self::degradation_notes(
                options,
                name,
                self.providers[name].capabilities(options),
            ));
        }
        if answered.len() < asked {
            notes.push(fanout_partial(answered.len(), asked));
        }
        if duplicates > 0 {
            notes.push(merged_duplicates(duplicates));
        }
        if results.is_empty() {
            notes.push(all_empty(answered.len()));
        }

        Ok(SearchAnswer {
            results,
            // Not one name any more. Joined rather than "several backends"
            // because `provider_used` is read to answer "who said this", and
            // the per-result `provider` fields it summarises are these names.
            provider: answered.join("+"),
            notes,
        })
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

    /// Execute search, honouring explicitly named backends or falling back
    /// through the configured chain.
    ///
    /// When `options.providers` names backends, only those are consulted: an
    /// unknown or unavailable name is a hard failure, never a silent fallback
    /// to a backend the caller did not choose. Naming several asks all of
    /// them concurrently and merges the answers (`fan_out`). Otherwise the
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
        // Naming backends is an instruction, not a preference: resolve them
        // and delegate, without touching the fallback chain below. One name
        // is the degenerate fan-out — same resolution rule, same failure
        // rule, so "you named a backend that is not there" cannot come out
        // differently depending on how many you named.
        if !options.providers.is_empty() {
            return self.fan_out(query, options).await;
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
                // Not recorded against health: this costs no round trip, and
                // `is_available` will keep answering false until the operator
                // fixes the configuration, so a demotion would buy nothing and
                // expire on a timer that has nothing to do with the cause.
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
                    let have = provider.capabilities(options);
                    let results = Self::attributed(&provider_name, results);
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
                    //
                    // Deliberately no `note_failure` here, and the absence is
                    // the point: a backend that says "I found nothing" gave a
                    // real answer to this query, so demoting it would push a
                    // working backend behind the others on the strength of one
                    // unlucky search term. (The SERP fallback *does* cool a
                    // mirror down on zero results — its mirrors are scrapers,
                    // where an empty page means the parser stopped matching,
                    // not that the web is empty. Same word, different fact.)
                    empty.push(provider_name);
                }
                Err(e) => {
                    let kind = classify_search_error(&e);
                    self.health.note_failure(&provider_name, &e);
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
            None if errors.is_empty() => {
                // Nothing was attempted at all: no candidate resolved to a
                // configured backend. "All providers failed" would be a report
                // about a chain that does not exist, and it tells the reader
                // to look for a failure instead of for a setting.
                Err(AlephError::invalid_config(
                    "no search backend is configured: add a backend under [search] in \
                     config.toml, or set TAVILY_API_KEY in the environment",
                ))
            }
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
                let have = self.providers[first].capabilities(options);
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

    /// Zero-config still works: a machine with TAVILY_API_KEY and no [search]
    /// block gets a one-backend registry, not a second code path that ignores
    /// every option the tool face accepts.
    #[test]
    fn an_env_only_install_still_gets_a_registry() {
        let reg = SearchRegistry::from_env_only("tvly-test").expect("registry");
        assert_eq!(reg.default_options().max_results, 5);
        assert!(reg.get_provider("tavily").is_some());
    }

    #[test]
    fn no_key_and_no_config_yields_no_registry_rather_than_a_second_path() {
        assert!(SearchRegistry::from_env_only("").is_none());
        assert!(
            SearchRegistry::from_env_only("   ").is_none(),
            "an all-whitespace key is unset, not a credential"
        );
    }

    /// The tool is built either way, so `for_tool` must answer for all three
    /// worlds. The one it used to get wrong is the third: no registry and no
    /// key produced a tool that read the environment itself.
    #[test]
    fn for_tool_prefers_the_configured_registry_then_the_key_then_nothing() {
        let configured = Arc::new(SearchRegistry::new("searxng"));
        assert_eq!(
            SearchRegistry::for_tool(Some(&configured), Some("tvly-test")).default_provider,
            "searxng",
            "a configured registry wins over a bare key"
        );
        assert_eq!(
            SearchRegistry::for_tool(None, Some("tvly-test")).default_provider,
            "tavily"
        );
        let empty = SearchRegistry::for_tool(None, None);
        assert!(
            empty.providers.is_empty(),
            "nothing configured must still yield a registry, just an empty one"
        );
    }

    /// Mock provider for testing
    struct MockProvider {
        name: String,
        should_fail: bool,
        result_count: usize,
        capabilities: SearchCapabilities,
        /// Host the mock's urls are minted under. Defaults to a shared one so
        /// two mocks look like two backends that found the same pages, which
        /// is the case merging exists for; `with_own_pages` makes them
        /// disagree instead.
        host: String,
        /// Fail with `Cancelled` rather than a network error, so a test can
        /// tell "the caller went away" from "the backend misbehaved".
        cancelled: bool,
        /// How many times this backend was actually asked. Ordering tests
        /// assert on this rather than on the returned results: a demoted
        /// backend that still gets a request has not been demoted, and the
        /// answer looks identical either way.
        asks: crate::sync_primitives::AtomicUsize,
    }

    impl MockProvider {
        fn new(name: &str, should_fail: bool, result_count: usize) -> Self {
            Self {
                name: name.to_string(),
                should_fail,
                result_count,
                capabilities: SearchCapabilities::default(),
                host: "example.com".to_string(),
                cancelled: false,
                asks: crate::sync_primitives::AtomicUsize::new(0),
            }
        }

        /// Fail as a cancellation instead of a network error.
        fn cancelling(mut self) -> Self {
            self.cancelled = true;
            self
        }

        /// Declares `recency` support, for capability-ordering tests.
        fn with_recency(mut self) -> Self {
            self.capabilities.recency = true;
            self
        }

        fn asks(&self) -> usize {
            self.asks.load(crate::sync_primitives::Ordering::Relaxed)
        }

        /// Declares `domain_filter` support, for capability-ordering tests.
        fn with_domain_filter(mut self) -> Self {
            self.capabilities.domain_filter = true;
            self
        }

        /// Mint urls under this backend's own host, so its results are
        /// distinct from another mock's.
        fn with_own_pages(mut self) -> Self {
            self.host = format!("{}.test", self.name);
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

        fn capabilities(&self, _options: &SearchOptions) -> SearchCapabilities {
            self.capabilities
        }

        async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
            self.asks
                .fetch_add(1, crate::sync_primitives::Ordering::Relaxed);
            if self.should_fail {
                return Err(if self.cancelled {
                    AlephError::Cancelled
                } else {
                    AlephError::network("Mock provider failure")
                });
            }

            let mut results = Vec::new();
            for i in 0..self.result_count.min(options.max_results) {
                results.push(SearchResult {
                    title: format!("{} - Result {}", query, i + 1),
                    url: format!("https://{}/{}", self.host, i + 1),
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
        assert!(
            err_msg.contains("no search backend is configured"),
            "nothing was attempted, so the message must name a setting, not a failure: {err_msg}"
        );
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
            providers: vec!["nope".into()],
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
            providers: vec!["a".into()],
            ..Default::default()
        };
        assert!(
            reg.search("q", &opts).await.is_err(),
            "must not silently use b"
        );
    }

    /// The request-aware capability bit has to reach the *sort*, not just the
    /// note — otherwise the backend that cannot express the constraint is
    /// still the one that gets asked first, and the note is an apology
    /// attached to the wrong answer.
    ///
    /// Uses the real `BingProvider` because Bing is the live instance of a
    /// bit that depends on the request. `ordered_candidates` touches no
    /// network: it reads capabilities and sorts.
    #[test]
    fn a_backend_sorts_behind_for_the_one_recency_value_it_cannot_express() {
        use crate::search::providers::BingProvider;
        let mut reg = SearchRegistry::new("bing");
        reg.add_provider("bing".into(), Arc::new(BingProvider::new("k").unwrap()));
        reg.add_provider(
            "everything".into(),
            Arc::new(MockProvider {
                capabilities: SearchCapabilities {
                    domain_filter: true,
                    recency: true,
                    full_content: true,
                },
                ..MockProvider::new("everything", false, 1)
            }),
        );
        reg.set_fallback_providers(vec!["everything".into()]);

        let order = |r: Option<crate::search::Recency>| {
            reg.ordered_candidates(&SearchOptions {
                recency: r,
                ..Default::default()
            })
        };
        assert_eq!(
            order(Some(crate::search::Recency::Week))[0],
            "bing",
            "bing is the configured default and carries Week, so it stays first"
        );
        assert_eq!(
            order(Some(crate::search::Recency::Year))[0],
            "everything",
            "for the bucket bing cannot express, a backend that can goes first"
        );
    }

    // ─── Fan-out across several named backends (Round-2) ─────────────────

    /// Naming two backends asks both and returns one set — the point of the
    /// feature. Interleaved by rank, so neither backend's best result is
    /// buried behind the other's worst.
    #[tokio::test]
    async fn naming_several_backends_asks_all_of_them_and_merges_the_answers() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider(
            "a".into(),
            Arc::new(MockProvider::new("a", false, 2).with_own_pages()),
        );
        reg.add_provider(
            "b".into(),
            Arc::new(MockProvider::new("b", false, 2).with_own_pages()),
        );
        let opts = SearchOptions {
            providers: vec!["a".into(), "b".into()],
            max_results: 10,
            ..Default::default()
        };
        let answer = reg.search("q", &opts).await.unwrap();
        assert_eq!(answer.results.len(), 4);
        assert_eq!(answer.provider, "a+b");
        let by_provider: Vec<&str> = answer
            .results
            .iter()
            .map(|r| r.provider.as_deref().unwrap_or("?"))
            .collect();
        assert_eq!(
            by_provider,
            vec!["a", "b", "a", "b"],
            "rank-interleaved, not concatenated"
        );
    }

    /// Two backends that found the same page produce one row, and the answer
    /// says how many it collapsed — otherwise "I asked for four and got two"
    /// is indistinguishable from "there are only two".
    #[tokio::test]
    async fn pages_both_backends_found_are_merged_and_counted() {
        let mut reg = SearchRegistry::new("a");
        // Both mocks mint the same urls: the same two pages, found twice.
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 2)));
        reg.add_provider("b".into(), Arc::new(MockProvider::new("b", false, 2)));
        let opts = SearchOptions {
            providers: vec!["a".into(), "b".into()],
            max_results: 10,
            ..Default::default()
        };
        let answer = reg.search("q", &opts).await.unwrap();
        assert_eq!(answer.results.len(), 2);
        assert!(
            answer.notes.iter().any(|n| n.contains("merged")),
            "the merge has to be said out loud: {:?}",
            answer.notes
        );
    }

    /// Backends disagree about what exists — that is why a caller asks
    /// several. One of them failing narrows the answer; it does not end it.
    #[tokio::test]
    async fn one_backend_failing_narrows_the_answer_rather_than_ending_it() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0)));
        reg.add_provider(
            "b".into(),
            Arc::new(MockProvider::new("b", false, 2).with_own_pages()),
        );
        let opts = SearchOptions {
            providers: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        let answer = reg.search("q", &opts).await.unwrap();
        assert_eq!(answer.results.len(), 2);
        assert_eq!(answer.provider, "b");
        assert!(
            answer.notes.iter().any(|n| n.contains("1 of 2")),
            "a caller who named two backends is the one person who can act on \
             which of them is down: {:?}",
            answer.notes
        );
    }

    /// Every named backend failing is still a failure, reported in the same
    /// `name [kind] message` shape the chain uses.
    #[tokio::test]
    async fn every_named_backend_failing_is_an_error_not_an_empty_answer() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0)));
        reg.add_provider("b".into(), Arc::new(MockProvider::new("b", true, 0)));
        let err = reg
            .search(
                "q",
                &SearchOptions {
                    providers: vec!["a".into(), "b".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("a ["), "{err}");
        assert!(err.contains("b ["), "{err}");
    }

    /// One unresolvable name fails the whole call even when the others are
    /// fine: the caller asked for those backends, and answering with a subset
    /// while reporting success is the confident wrong answer this rule
    /// exists to prevent. Both missing names are listed so a caller fixing a
    /// typo does not have to re-run to find the second one.
    #[tokio::test]
    async fn an_unknown_name_among_several_fails_the_call_and_lists_them_all() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 2)));
        let err = reg
            .search(
                "q",
                &SearchOptions {
                    providers: vec!["a".into(), "nope".into(), "alsonope".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("alsonope"), "{err}");
        assert!(
            err.contains("configured: a"),
            "the error must list what IS configured: {err}"
        );
    }

    /// Naming a backend twice asks it once. Without the dedup its quota is
    /// spent twice and every one of its results arrives as its own duplicate,
    /// which would then be reported as backends agreeing.
    #[tokio::test]
    async fn naming_the_same_backend_twice_asks_it_once() {
        let mut reg = SearchRegistry::new("a");
        reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 2)));
        let answer = reg
            .search(
                "q",
                &SearchOptions {
                    providers: vec!["a".into(), "a".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(answer.provider, "a");
        assert!(
            !answer.notes.iter().any(|n| n.contains("merged")),
            "asking one backend cannot produce cross-backend duplicates: {:?}",
            answer.notes
        );
    }

    /// Rows and the summary have to name backends in the same vocabulary.
    ///
    /// Providers stamp their own *type* (`searxng`); callers address the
    /// *configuration key* (`alpha`). They coincide on the usual config, so a
    /// fan-out over two instances of one provider type is the first place the
    /// difference shows — and it is the exact case per-row attribution exists
    /// for. Caught by `qa/web_search/run.sh fanout` on its first run, with
    /// `provider_used: "alpha+bravo"` over rows all saying `searxng`.
    #[tokio::test]
    async fn rows_name_the_backend_the_caller_named_not_the_provider_type() {
        let mut reg = SearchRegistry::new("alpha");
        // Both mocks report their own name as `shared-type`, standing in for
        // two backends of one provider type.
        reg.add_provider(
            "alpha".into(),
            Arc::new(MockProvider {
                name: "shared-type".into(),
                ..MockProvider::new("alpha", false, 1).with_own_pages()
            }),
        );
        reg.add_provider(
            "bravo".into(),
            Arc::new(MockProvider {
                name: "shared-type".into(),
                host: "bravo.test".into(),
                ..MockProvider::new("bravo", false, 1)
            }),
        );
        let answer = reg
            .search(
                "q",
                &SearchOptions {
                    providers: vec!["alpha".into(), "bravo".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut names: Vec<&str> = answer
            .results
            .iter()
            .map(|r| r.provider.as_deref().unwrap_or("?"))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["alpha", "bravo"],
            "rows must use the names `provider_used` ({}) and `providers` use",
            answer.provider
        );
    }

    /// The chain path uses the same vocabulary: a backend configured under a
    /// name that is not its provider type must be reported under the name.
    #[tokio::test]
    async fn the_chain_also_reports_the_configured_name() {
        let mut reg = SearchRegistry::new("primary");
        reg.add_provider(
            "primary".into(),
            Arc::new(MockProvider {
                name: "shared-type".into(),
                ..MockProvider::new("primary", false, 1)
            }),
        );
        let answer = reg.search("q", &SearchOptions::default()).await.unwrap();
        assert_eq!(answer.provider, "primary");
        assert_eq!(answer.results[0].provider.as_deref(), Some("primary"));
    }

    /// A dimension is dropped per backend, not per answer: in a merged set
    /// one half can be domain-filtered and the other not, and a single note
    /// naming one backend would describe the wrong half of the results.
    #[tokio::test]
    async fn each_backend_that_cannot_carry_a_dimension_says_so_under_its_own_name() {
        // Names chosen so neither appears inside the note's own prose: the
        // first draft called the capable backend `filtered`, and the note
        // about the OTHER backend ends "unfiltered on that axis" — the
        // assertion matched its own sentence and reported a defect that was
        // not there.
        let mut reg = SearchRegistry::new("alpha");
        reg.add_provider(
            "alpha".into(),
            Arc::new(
                MockProvider::new("alpha", false, 1)
                    .with_domain_filter()
                    .with_own_pages(),
            ),
        );
        reg.add_provider(
            "bravo".into(),
            Arc::new(MockProvider::new("bravo", false, 1).with_own_pages()),
        );
        let answer = reg
            .search(
                "q",
                &SearchOptions {
                    providers: vec!["alpha".into(), "bravo".into()],
                    include_domains: vec!["docs.rs".into()],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let degraded: Vec<&String> = answer
            .notes
            .iter()
            .filter(|n| n.contains("was not applied"))
            .collect();
        assert_eq!(degraded.len(), 1, "{:?}", answer.notes);
        assert!(degraded[0].contains("`bravo`"), "{}", degraded[0]);
        assert!(
            !degraded[0].contains("`alpha`"),
            "the backend that DID filter must not be named as having dropped it: {}",
            degraded[0]
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

    // ---- health: a recent failure demotes, it never silences -------------

    /// Two-backend chain: `first` is the default, `second` the fallback.
    /// Returns the registry; the callers keep their own `Arc`s so they can
    /// read each mock's ask counter afterwards.
    fn chain(first: &Arc<MockProvider>, second: &Arc<MockProvider>) -> SearchRegistry {
        let (a, b) = (first.name.clone(), second.name.clone());
        let mut registry = SearchRegistry::new(a.clone());
        registry.add_provider(a, Arc::clone(first) as Arc<dyn SearchProvider>);
        registry.add_provider(b.clone(), Arc::clone(second) as Arc<dyn SearchProvider>);
        registry.set_fallback_providers(vec![b]);
        registry
    }

    /// The anti-vacuous one: with nobody degraded the order must be exactly
    /// what it was before health existed, so the guards below are measuring
    /// the demotion rather than some incidental reshuffle.
    #[tokio::test]
    async fn health_moves_nothing_until_something_has_failed() {
        let a = Arc::new(MockProvider::new("a", false, 1));
        let b = Arc::new(MockProvider::new("b", false, 1));
        let registry = chain(&a, &b);
        let options = SearchOptions::default();

        assert_eq!(
            registry.ordered_candidates(&options),
            vec!["a".to_string(), "b".to_string()],
            "an all-healthy chain must be the configured order, untouched"
        );

        registry
            .health
            .note_failure("a", &AlephError::rate_limit("429"));
        assert_eq!(
            registry.ordered_candidates(&options),
            vec!["b".to_string(), "a".to_string()],
            "the backend that just refused goes last in its group"
        );

        registry.health.clear();
        assert_eq!(
            registry.ordered_candidates(&options),
            vec!["a".to_string(), "b".to_string()],
            "and comes back once the demotion has expired"
        );
    }

    /// The whole point: a dead backend at the head of the configured order
    /// stops costing a full timeout on every subsequent search.
    ///
    /// Asserted on the mock's ask counter, not on the answer — a demoted
    /// backend that is still being asked produces a byte-identical answer,
    /// so the results cannot tell the two apart.
    #[tokio::test]
    async fn a_backend_that_failed_is_not_reached_again_while_one_ahead_answers() {
        let bad = Arc::new(MockProvider::new("bad", true, 0));
        let good = Arc::new(MockProvider::new("good", false, 2));
        let registry = chain(&bad, &good);
        let options = SearchOptions::default();

        let first = registry.search("q", &options).await.unwrap();
        assert_eq!(first.provider, "good");
        assert_eq!(bad.asks(), 1, "the first search pays the failure once");
        assert!(
            first.notes.iter().any(|n| n.contains("failed")),
            "{:?}",
            first.notes
        );

        let second = registry.search("q", &options).await.unwrap();
        assert_eq!(second.provider, "good");
        assert_eq!(
            bad.asks(),
            1,
            "the second search must not pay it again while `good` answers"
        );
        assert_eq!(good.asks(), 2);
        assert!(
            !second.notes.iter().any(|n| n.contains("failed")),
            "nothing failed this time, so the answer must not say it did: {:?}",
            second.notes
        );
    }

    /// Load-bearing: capability outranks health.
    ///
    /// `capable` is the only backend that can express the requested
    /// dimension, and it has just failed. If health were allowed to move it,
    /// `plain` would answer, drop the dimension, and explain itself with a
    /// note naming its own missing parameter — true, and pointing at the
    /// wrong cause. A demotion must never buy a worse answer.
    #[tokio::test]
    async fn a_recent_failure_does_not_outrank_the_only_backend_that_can_carry_the_request() {
        let capable = Arc::new(MockProvider::new("capable", true, 0).with_recency());
        let plain = Arc::new(MockProvider::new("plain", false, 2));
        let registry = chain(&capable, &plain);
        let asks_for_recency = SearchOptions {
            recency: Some(crate::search::Recency::Week),
            ..Default::default()
        };

        registry.search("q", &asks_for_recency).await.unwrap();
        assert_eq!(capable.asks(), 1);
        registry.search("q", &asks_for_recency).await.unwrap();
        assert_eq!(
            capable.asks(),
            2,
            "health may only break ties inside a capability group"
        );
        assert!(
            registry.health.is_degraded("capable"),
            "the failure was recorded — it just is not allowed to win here"
        );
    }

    /// "I found nothing" is an answer to this query, not a fault in the
    /// backend. Demoting on it would push a working backend behind the others
    /// on the strength of one unlucky search term.
    #[tokio::test]
    async fn a_backend_that_found_nothing_is_not_demoted() {
        let quiet = Arc::new(MockProvider::new("quiet", false, 0));
        let other = Arc::new(MockProvider::new("other", false, 2));
        let registry = chain(&quiet, &other);
        let options = SearchOptions::default();

        registry.search("q", &options).await.unwrap();
        registry.search("q", &options).await.unwrap();
        assert_eq!(quiet.asks(), 2, "an empty answer is still an answer");
        assert!(!registry.health.is_degraded("quiet"));
    }

    /// A cancellation reports that the *caller* went away mid-flight. Letting
    /// it demote would mean a user who interrupts twice reorders the chain.
    #[tokio::test]
    async fn a_cancelled_attempt_does_not_demote_the_backend() {
        let interrupted = Arc::new(MockProvider::new("interrupted", true, 0).cancelling());
        let other = Arc::new(MockProvider::new("other", false, 2));
        let registry = chain(&interrupted, &other);
        let options = SearchOptions::default();

        registry.search("q", &options).await.unwrap();
        registry.search("q", &options).await.unwrap();
        assert_eq!(
            interrupted.asks(),
            2,
            "the caller walking away says nothing about the backend"
        );
        assert!(!registry.health.is_degraded("interrupted"));
    }

    /// The two faces of the same verb: the fan-out **records** what it saw,
    /// so a backend only ever reached by name still teaches the chain — but
    /// it is never reordered or skipped, because naming a backend is an
    /// instruction and not a preference.
    #[tokio::test]
    async fn naming_a_backend_asks_it_however_it_last_behaved() {
        let flaky = Arc::new(MockProvider::new("flaky", true, 0));
        let good = Arc::new(MockProvider::new("good", false, 2).with_own_pages());
        let registry = chain(&flaky, &good);
        let named = SearchOptions {
            providers: vec!["flaky".to_string(), "good".to_string()],
            ..Default::default()
        };

        registry.search("q", &named).await.unwrap();
        assert_eq!(flaky.asks(), 1);
        assert!(
            registry.health.is_degraded("flaky"),
            "a failure is a failure whichever face observed it"
        );

        let answer = registry.search("q", &named).await.unwrap();
        assert_eq!(
            flaky.asks(),
            2,
            "a named backend is asked whatever its health"
        );
        assert_eq!(answer.provider, "good");
    }
}

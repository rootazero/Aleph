//! Search provider factory contract — decouples `SearchRegistry::from_config`
//! from the concrete provider set so that new providers can be wired in by
//! declaring a factory rather than by editing a hard-coded `match` arm.
//!
//! Design notes:
//! - Inspired by openclaw's `WebSearchProviderPlugin` contract
//!   (`src/web-search/runtime.ts`) but implemented as a Rust trait —
//!   we get compile-time dispatch + zero plugin-manifest overhead.
//! - The registry is built once at startup via [`ProviderFactoryRegistry::with_defaults`]
//!   and is intentionally not hot-pluggable; runtime registration of a
//!   new provider type would require recompilation.
//! - Factory `build` returns `Ok(None)` when the backend is configured
//!   but unusable (e.g., missing `api_key`) — callers log+skip rather than
//!   aborting the whole load, matching the prior behaviour of
//!   `SearchRegistry::from_config`.

use crate::config::types::SearchBackendConfig;
use crate::error::Result;
use crate::search::SearchProvider;
use crate::sync_primitives::Arc;
use std::collections::HashMap;

/// One entry in the provider factory table.
///
/// Each implementor knows how to construct exactly one concrete
/// `SearchProvider` from a `SearchBackendConfig` block. Factories are
/// stateless — the trait exists only to dispatch on `provider_type`
/// without committing the registry core to the concrete provider set.
pub trait ProviderFactory: Send + Sync {
    /// The `provider_type` string this factory handles
    /// (e.g. `"tavily"`, `"searxng"`).
    fn provider_type(&self) -> &'static str;

    /// Construct a provider instance.
    ///
    /// - `Ok(Some(provider))` — provider was constructed and is ready to
    ///   serve requests.
    /// - `Ok(None)` — backend is configured but **intentionally unusable**
    ///   (e.g. `provider_type = "tavily"` with no `api_key` in the vault).
    ///   The registry logs a warning at WARN level and continues with the
    ///   remaining backends; the search tool can still satisfy requests
    ///   via other providers or the legacy `TAVILY_API_KEY` env path.
    /// - `Err(_)` — hard construction failure (e.g. malformed `base_url`).
    ///   The registry treats this identically to `Ok(None)` for now
    ///   (warn + skip) but the distinct return shape lets future code
    ///   raise such errors to operators if desired.
    fn build(
        &self,
        name: &str,
        backend: &SearchBackendConfig,
    ) -> Result<Option<Arc<dyn SearchProvider>>>;
}

/// Indexed table of [`ProviderFactory`] implementations keyed by
/// `provider_type`.
///
/// Use [`ProviderFactoryRegistry::with_defaults`] to obtain the standard
/// table containing all 9 first-party providers. Tests may construct an
/// empty registry via [`ProviderFactoryRegistry::new`] and register a
/// subset.
pub struct ProviderFactoryRegistry {
    factories: HashMap<&'static str, Box<dyn ProviderFactory>>,
}

impl ProviderFactoryRegistry {
    /// Construct an empty registry. Mostly for tests.
    #[must_use]
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Default registry containing every first-party search provider.
    /// Edit this list when adding a new provider — the only required edit
    /// is one line here, plus the provider's own `pub struct XxxFactory;`.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(Box::new(crate::search::providers::TavilyFactory));
        r.register(Box::new(crate::search::providers::SearxngFactory));
        r.register(Box::new(crate::search::providers::BraveFactory));
        r.register(Box::new(crate::search::providers::BingFactory));
        r.register(Box::new(crate::search::providers::GoogleFactory));
        r.register(Box::new(crate::search::providers::ExaFactory));
        r.register(Box::new(crate::search::providers::FirecrawlFactory));
        r.register(Box::new(crate::search::providers::JinaFactory));
        r.register(Box::new(crate::search::providers::DuckDuckGoFactory));
        r
    }

    /// Register a new factory. Last-registered wins on duplicate keys.
    pub fn register(&mut self, factory: Box<dyn ProviderFactory>) {
        self.factories.insert(factory.provider_type(), factory);
    }

    /// Return the set of `provider_type` strings this registry can build.
    /// Used by the config validator so it stays in sync without a
    /// hard-coded allowlist.
    #[must_use]
    pub fn known_provider_types(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = self.factories.keys().copied().collect();
        v.sort();
        v
    }

    /// Construct a provider for the given backend entry.
    ///
    /// Returns:
    /// - `Ok(Some(provider))` on success
    /// - `Ok(None)` when the factory chose to skip (e.g. missing `api_key`)
    /// - `Ok(None)` when no factory is registered for the `provider_type`
    ///   (a typo or unknown provider — logged at WARN by caller)
    /// - `Err(_)` on hard construction failures
    pub fn build(
        &self,
        name: &str,
        backend: &SearchBackendConfig,
    ) -> Result<Option<Arc<dyn SearchProvider>>> {
        let Some(factory) = self.factories.get(backend.provider_type.as_str()) else {
            log::warn!(
                "search backend '{name}' has unknown provider_type '{}', skipped",
                backend.provider_type
            );
            return Ok(None);
        };
        factory.build(name, backend)
    }
}

impl Default for ProviderFactoryRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::SearchBackendConfig;

    fn backend(provider_type: &str) -> SearchBackendConfig {
        SearchBackendConfig {
            provider_type: provider_type.to_string(),
            api_key: None,
            base_url: None,
            engine_id: None,
            engines: None,
            min_request_interval_ms: None,
            verified: false,
            allow_private_upstream: false,
        }
    }

    /// Set equality, both directions.
    ///
    /// This replaces a hand-copied list of the nine names, which was a second
    /// statement of the same fact and could only ever assert containment: an
    /// entry missing from *both* the list and the registry reads as passing,
    /// which is how two channel adapters sat unconfigurable for four months.
    #[test]
    fn the_factory_builds_exactly_the_providers_the_protocol_advertises() {
        use std::collections::BTreeSet;
        let buildable: BTreeSet<&str> = ProviderFactoryRegistry::with_defaults()
            .known_provider_types()
            .into_iter()
            .collect();
        let advertised: BTreeSet<&str> = aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS
            .iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(
            buildable, advertised,
            "left = the factory can build it, right = the UI offers it. \
             An entry on only one side is either a provider nobody can configure \
             or a card that saves a config the server will refuse."
        );
    }

    #[test]
    fn unknown_provider_type_yields_none_without_error() {
        let reg = ProviderFactoryRegistry::with_defaults();
        let built = reg.build("typo", &backend("brrrrave")).unwrap();
        assert!(built.is_none());
    }

    #[test]
    fn tavily_without_api_key_skipped() {
        let reg = ProviderFactoryRegistry::with_defaults();
        let built = reg.build("primary", &backend("tavily")).unwrap();
        assert!(built.is_none(), "tavily should be skipped without api_key");
    }

    /// DuckDuckGo needs no api_key and no base_url — it must always
    /// successfully construct. This is the canary that "factory exists +
    /// factory wired into with_defaults()" stays correct.
    #[test]
    fn duckduckgo_constructs_without_credentials() {
        let reg = ProviderFactoryRegistry::with_defaults();
        let built = reg.build("ddg", &backend("duckduckgo")).unwrap();
        assert!(built.is_some(), "duckduckgo should construct without keys");
    }
}

//! Which search backends exist, in one place.
//!
//! There were three statements of this: the factory registry in
//! `alephcore::search`, a doc comment in `config/types/search.rs` listing nine
//! names, and the Panel's `PRESETS` table listing eight — `jina` had a
//! provider, a factory and a doc entry but no card, so the only way to
//! configure it was to hand-edit `config.toml`.
//!
//! Presentation (icon colour, prose description, i18n) stays in the Panel;
//! this constant owns identity and config shape, and a census on each side
//! asserts the two agree **as sets**. A one-way containment assertion reads as
//! passing when both sides are missing the same entry, which is how two
//! channel adapters stayed unconfigurable for four months.

/// A backend an operator can configure, and what its config needs.
///
/// The three `needs_*` flags are read from the factory that builds the
/// backend, not from what the vendor's API happens to accept: they answer
/// "will this config produce a working provider", which is the only question
/// a form can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchProviderPreset {
    /// The `provider_type` written into `[search.backends.*]`, and the name
    /// results are reported under.
    pub name: &'static str,
    /// Human-facing name. Not translated: these are product names.
    pub display_name: &'static str,
    /// The factory skips this backend without a key.
    pub needs_api_key: bool,
    /// The factory skips this backend without a `base_url` (self-hosted).
    pub needs_base_url: bool,
    /// The factory skips this backend without an `engine_id` (Google CSE).
    pub needs_engine_id: bool,
    /// What to prefill `base_url` with. `None` where the endpoint is fixed in
    /// the provider and the field is ignored.
    pub default_base_url: Option<&'static str>,
    /// A sample of the key's shape, for a form's placeholder. `None` where we
    /// have no documented prefix — a made-up example in this position teaches
    /// the operator a format that does not exist.
    pub api_key_placeholder: Option<&'static str>,
}

/// Every backend `ProviderFactoryRegistry::with_defaults` can build.
///
/// Order is the curated one the Panel shipped: most operators reach for the
/// hosted APIs first. It is not authoritative for anything but presentation —
/// the registry routes by configuration, never by this order.
pub const CONFIGURABLE_SEARCH_PROVIDERS: &[SearchProviderPreset] = &[
    SearchProviderPreset {
        name: "tavily",
        display_name: "Tavily",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: Some("https://api.tavily.com"),
        api_key_placeholder: Some("tvly-..."),
    },
    SearchProviderPreset {
        name: "brave",
        display_name: "Brave",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: Some("https://api.search.brave.com/res/v1"),
        api_key_placeholder: Some("BSA..."),
    },
    SearchProviderPreset {
        name: "google",
        display_name: "Google",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: true,
        default_base_url: Some("https://www.googleapis.com/customsearch/v1"),
        api_key_placeholder: Some("AIza..."),
    },
    SearchProviderPreset {
        name: "bing",
        display_name: "Bing",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: Some("https://api.bing.microsoft.com/v7.0"),
        api_key_placeholder: Some("Ocp-Apim..."),
    },
    SearchProviderPreset {
        name: "searxng",
        display_name: "SearXNG",
        needs_api_key: false,
        needs_base_url: true,
        needs_engine_id: false,
        default_base_url: Some("http://localhost:8080"),
        api_key_placeholder: None,
    },
    SearchProviderPreset {
        name: "exa",
        display_name: "Exa",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: Some("https://api.exa.ai"),
        api_key_placeholder: Some("exa-..."),
    },
    SearchProviderPreset {
        name: "firecrawl",
        display_name: "Firecrawl",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: Some("https://api.firecrawl.dev"),
        api_key_placeholder: Some("fc-..."),
    },
    SearchProviderPreset {
        name: "duckduckgo",
        display_name: "DuckDuckGo",
        needs_api_key: false,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: None,
        api_key_placeholder: None,
    },
    SearchProviderPreset {
        name: "jina",
        display_name: "Jina",
        needs_api_key: true,
        needs_base_url: false,
        needs_engine_id: false,
        default_base_url: None,
        api_key_placeholder: None,
    },
];

/// Look a preset up by the name written in `provider_type`.
#[must_use]
pub fn preset(name: &str) -> Option<&'static SearchProviderPreset> {
    CONFIGURABLE_SEARCH_PROVIDERS
        .iter()
        .find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two entries with one name would make `preset()` answer for whichever
    /// came first, and the census on either side would still pass as sets.
    #[test]
    fn every_preset_name_is_unique() {
        let mut names: Vec<&str> = CONFIGURABLE_SEARCH_PROVIDERS
            .iter()
            .map(|p| p.name)
            .collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            total,
            "duplicate provider name in the preset table"
        );
    }

    /// A backend that needs nothing to be configured is reachable with an
    /// empty form; one that needs a key must say so, or the form saves a
    /// config the factory will skip with only a log line to show for it.
    #[test]
    fn a_preset_that_needs_a_key_offers_no_default_that_hides_it() {
        for p in CONFIGURABLE_SEARCH_PROVIDERS {
            if p.needs_base_url {
                assert!(
                    p.default_base_url.is_some(),
                    "{}: a required base_url with nothing to prefill is a form with a blank \
                     mandatory field and no hint",
                    p.name
                );
            }
        }
    }
}

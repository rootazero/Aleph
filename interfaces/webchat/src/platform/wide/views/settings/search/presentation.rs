use crate::api::SearchBackendEntry;

// ============================================================================
// Preset Definitions
// ============================================================================

/// Everything about a backend that is presentation, and therefore ours.
///
/// Identity and config shape come from
/// `aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS`: the Panel may
/// style a backend however it likes, but it may not decide which backends
/// exist. It used to, and the answer had drifted — `jina` has a provider, a
/// factory and a line in the config docs, and had no card here, so the only
/// way to configure it was to hand-edit config.toml.
pub(super) struct SearchPresentation {
    name: &'static str,
    description: &'static str,
    icon_color: &'static str,
    /// Whether the operator runs this backend themselves. Distinct from the
    /// protocol's `needs_base_url` even though today the same one backend has
    /// both: that one says the config will be rejected without a URL, this
    /// one changes the hint we show next to the field.
    is_self_hosted: bool,
}

pub(super) const PRESENTATION: &[SearchPresentation] = &[
    SearchPresentation {
        name: "tavily",
        description: "AI-powered search API",
        icon_color: "#5B5FC7",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "brave",
        description: "Brave Search API",
        icon_color: "#FB542B",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "google",
        description: "Google Custom Search",
        icon_color: "#4285F4",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "bing",
        description: "Bing Web Search API",
        icon_color: "#008373",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "searxng",
        description: "Self-hosted meta search",
        icon_color: "#3050FF",
        is_self_hosted: true,
    },
    SearchPresentation {
        name: "exa",
        description: "Neural search engine",
        icon_color: "#000000",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "firecrawl",
        description: "Search + full-content scraping",
        icon_color: "#FF6B35",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "duckduckgo",
        description: "No-account HTML search",
        icon_color: "#DE5833",
        is_self_hosted: false,
    },
    SearchPresentation {
        name: "jina",
        description: "LLM-ready snippets",
        icon_color: "#E33E3E",
        is_self_hosted: false,
    },
];

/// Neutral styling for a backend the protocol advertises and this table has
/// not been taught about yet.
///
/// Deliberately not a skip: a new backend with no card is a backend nobody
/// can configure, which is the failure this whole split exists to remove. A
/// census below asserts the fallback is unused today.
pub(super) const UNSTYLED: SearchPresentation = SearchPresentation {
    name: "",
    description: "",
    icon_color: "#6B7280",
    is_self_hosted: false,
};

/// One backend's card, identity and styling joined.
#[derive(Clone, Copy)]
pub(super) struct SearchPreset {
    pub(super) name: &'static str,
    pub(super) display_name: &'static str,
    pub(super) description: &'static str,
    /// What to prefill the base URL field with; empty when the endpoint is
    /// fixed in the provider.
    pub(super) base_url: &'static str,
    /// Empty when we have no documented key shape to suggest.
    pub(super) api_key_placeholder: &'static str,
    pub(super) icon_color: &'static str,
    pub(super) needs_api_key: bool,
    pub(super) is_self_hosted: bool,
    pub(super) needs_engine_id: bool,
}

pub(super) fn join(preset: &'static aleph_protocol::search::SearchProviderPreset) -> SearchPreset {
    let style = PRESENTATION
        .iter()
        .find(|p| p.name == preset.name)
        .unwrap_or(&UNSTYLED);
    SearchPreset {
        name: preset.name,
        display_name: preset.display_name,
        description: style.description,
        base_url: preset.default_base_url.unwrap_or(""),
        api_key_placeholder: preset.api_key_placeholder.unwrap_or(""),
        icon_color: style.icon_color,
        needs_api_key: preset.needs_api_key,
        is_self_hosted: style.is_self_hosted,
        needs_engine_id: preset.needs_engine_id,
    }
}

/// Every backend the server can build, in the protocol's order.
pub(super) fn presets() -> impl Iterator<Item = SearchPreset> {
    aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS
        .iter()
        .map(join)
}

pub(super) fn find_preset(name: &str) -> Option<SearchPreset> {
    aleph_protocol::search::preset(name).map(join)
}

/// Find backend entry for a provider name from the config's backends list
pub(super) fn find_backend<'a>(
    backends: &'a [SearchBackendEntry],
    name: &str,
) -> Option<&'a SearchBackendEntry> {
    backends.iter().find(|b| b.name == name)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The Panel may style a backend however it likes, but it may not decide
    /// which backends exist. Set equality in both directions: a name on only
    /// one side is either a backend with no card or a card that saves a
    /// config the server will refuse.
    #[test]
    fn every_advertised_provider_has_a_card() {
        use std::collections::BTreeSet;
        let carded: BTreeSet<&str> = PRESENTATION.iter().map(|p| p.name).collect();
        let advertised: BTreeSet<&str> = aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS
            .iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(carded, advertised);
    }

    /// The fallback exists so a backend added server-side is still
    /// configurable before anyone styles it. If it is ever actually in use,
    /// the card above it is blank-looking and this says so.
    #[test]
    fn no_advertised_provider_falls_back_to_the_unstyled_card() {
        for p in presets() {
            assert!(
                !p.description.is_empty(),
                "{} renders with the neutral placeholder card",
                p.name
            );
        }
    }

    /// A card's key field is only useful with the shape the backend expects,
    /// and its URL field is only prefillable where there is a URL to prefill.
    #[test]
    fn the_join_carries_the_protocol_side_of_each_card() {
        let searxng = find_preset("searxng").expect("searxng has a card");
        assert!(!searxng.needs_api_key);
        assert!(searxng.is_self_hosted);
        assert_eq!(searxng.base_url, "http://localhost:8080");

        let google = find_preset("google").expect("google has a card");
        assert!(google.needs_engine_id);

        let jina = find_preset("jina").expect("jina finally has a card");
        assert!(jina.needs_api_key);
        assert_eq!(
            jina.base_url, "",
            "a fixed endpoint must not prefill a field the provider ignores"
        );
    }
}

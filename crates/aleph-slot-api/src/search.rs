//! Search contract — the pure data/trait half of `alephcore::search`.
//!
//! This module owns the search *contract* surface that Task 3 of the P0
//! modularization moved out of `alephcore` (where it lived as
//! `src/search/options.rs`, `src/search/result.rs`, and
//! `src/search/provider.rs`):
//!
//! * [`Recency`] and [`SearchOptions`] — the request options, including the
//!   per-provider parameter mappers for the four "rich" fields
//!   (`language`, `region`, `recency`, `safe_search`). Each provider
//!   expects different field names and value vocabularies for the same
//!   concept — by collecting the mappings here, the per-provider HTTP
//!   builders (which stay in `alephcore::search::providers`) stay simple
//!   (each just calls one helper) and adding a new provider requires
//!   extending this mapping table rather than reverse-engineering the
//!   convention from a sibling provider.
//! * [`SearchResult`] — the unified result shape all providers return.
//! * [`SearchCapabilities`] and [`SearchProvider`] — the provider trait and
//!   its per-request capability bits.
//!
//! Everything that performs I/O stays in `alephcore`: the nine provider
//! HTTP implementations, the registry/factory/handle, URL validation, and
//! configuration. `alephcore::search::{options, provider, result}` are
//! compatibility re-export shims pointing here, not second definitions.
//!
//! Mapping table (kept in sync with the helpers below):
//!
//! | field        | Brave         | Bing         | Google CSE   | `SearXNG`      | Tavily   | `DuckDuckGo` | Firecrawl | Exa | Jina |
//! |--------------|---------------|--------------|--------------|----------------|----------|--------------|-----------|-----|------|
//! | language     | `search_lang`   | setLang      | `lr=lang_XX`   | language     | —        | —          | lang      | —   | —    |
//! | region       | country       | cc           | gl           | —            | —        | kl         | country   | —   | —    |
//! | `recency`      | freshness     | freshness    | dateRestrict | `time_range`   | days     | df         | tbs       | —   | —    |
//! | `safe_search`  | safesearch    | safeSearch   | safe         | safesearch   | —        | kp         | —         | —   | —    |
//!
//! Providers that have no native concept for a field omit it entirely
//! (the helper returns `None` or the call site simply doesn't push it).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ============================================================================
// Recency + SearchOptions (moved from `src/search/options.rs`)
// ============================================================================

/// The freshness vocabulary, owned in one place.
///
/// Every provider has its own spelling for the same four buckets (`pd` /
/// `Day` / `d1` / `qdr:d` / 1 day / ...). Before this enum the four words
/// lived seven times over — once inside each mapper — and every mapper
/// answered `None` for anything it did not recognise, so a caller passing
/// `"7d"` got an unconstrained search while believing it had constrained one.
/// With a closed enum the rejection happens at the tool boundary and names
/// the legal values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Recency {
    Day,
    Week,
    Month,
    Year,
}

/// Search options passed to providers.
///
/// See module-level docs for the per-provider mapping table covering
/// `language`/`region`/`recency`/`safe_search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Language code (ISO 639-1: "en", "zh", "ja", etc.)
    /// Forwarded to Brave/Bing/Google/SearXNG; ignored by Tavily/Exa/Jina/DDG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Region code (ISO 3166-1 alpha-2: "US", "CN", "JP", etc.)
    /// Forwarded to Brave/Bing/Google/DDG; ignored by Tavily/SearXNG/Exa/Jina.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// How fresh a result has to be. `None` = no constraint.
    /// Forwarded to Brave/Bing/Google/SearXNG/Tavily/DDG/Firecrawl, each in
    /// its own vocabulary — see the mapping table above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<Recency>,

    /// Enable safe search (adult content filtering)
    /// Forwarded to Brave/Bing/Google/SearXNG/DDG; ignored by Tavily/Exa/Jina.
    #[serde(default = "default_safe_search")]
    pub safe_search: bool,

    /// Maximum number of results (default: 5)
    #[serde(default = "default_max_results")]
    pub max_results: usize,

    /// Timeout in seconds (default: 10)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Include full page content (Tavily only)
    /// WARNING: Significantly increases latency and token usage
    #[serde(default)]
    pub include_full_content: bool,

    /// Restrict results to these domains. Empty = no constraint.
    ///
    /// Not every backend has a native parameter for this; the ones that do
    /// declare `SearchCapabilities::domain_filter` and the registry prefers
    /// them. A backend that cannot express it says so in the result notes
    /// rather than silently returning the whole web.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_domains: Vec<String>,

    /// Drop results from these domains. Empty = no constraint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,

    /// Consult exactly these backends. Empty = the configured chain.
    ///
    /// Naming any is an instruction, not a preference: if one is unknown or
    /// unavailable the search fails and says so, rather than quietly
    /// answering from a backend the caller did not pick.
    ///
    /// Three behaviours in one field, and the list length is what picks:
    ///
    /// * empty — the operator's default provider, then `fallback_providers`,
    ///   stopping at the first backend that answers;
    /// * one name — that backend alone;
    /// * several — all of them, concurrently, merged into one set with
    ///   `search::merge` (in `alephcore`) deciding what counts as the same
    ///   result.
    ///
    /// It is a list rather than an `Option<String>` plus a second "breadth"
    /// switch because those would be two fields naming one concept, and the
    /// caller would have to be told which wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,

    /// How much of a snippet the caller will actually keep, in characters.
    /// `None` = no bound, which is what every caller but the tool face wants.
    ///
    /// Exists because one backend can honour it *server-side*: Exa has no
    /// snippet field at all, only whole page bodies, so without a cap it
    /// downloads and bills for an entire page per result and the caller keeps
    /// a paragraph. Asking Exa for the caller's budget makes the cost match
    /// what is used.
    ///
    /// It is a number the **caller** owns, not a `[search]` knob: the bound
    /// belongs to whoever renders the snippet, and a provider hardcoding it
    /// would be that same fact written down twice — which only stays true
    /// until one of them changes.
    ///
    /// Callers should pass their budget **plus one**. A backend told to
    /// return exactly the budget makes "the page was this long" and "the page
    /// was cut here" the same observation, and the note that offers to fetch
    /// the rest would stop firing for a page that really does have more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet_budget_chars: Option<usize>,
}

/// The single upper bound for `max_results`, wherever it is validated.
///
/// Three places enforce or warn on the same ceiling —
/// [`SearchOptions::validated_max_results`] (the request-side clamp), the
/// `search_config.update` gateway handler (the Panel's edit boundary), and
/// `config::validate`'s load-time advisory. The value is 100 because that is
/// what the user-facing configuration boundary has always allowed; the
/// request-side clamp used to silently cap at 50, so a Panel-saved
/// `max_results = 80` was accepted, persisted, and quietly served as 50.
pub const MAX_SEARCH_RESULTS: usize = 100;

const fn default_safe_search() -> bool {
    true
}

const fn default_max_results() -> usize {
    5
}

const fn default_timeout() -> u64 {
    10
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            language: None,
            region: None,
            recency: None,
            safe_search: default_safe_search(),
            max_results: default_max_results(),
            timeout_seconds: default_timeout(),
            include_full_content: false,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            providers: Vec::new(),
            snippet_budget_chars: None,
        }
    }
}

impl SearchOptions {
    /// Create default options with custom timeout
    #[must_use]
    pub fn default_with_timeout(timeout_seconds: u64) -> Self {
        Self {
            timeout_seconds,
            ..Default::default()
        }
    }

    /// Returns a validated timeout in seconds, ensuring it's at least 1
    #[must_use]
    pub fn validated_timeout(&self) -> u64 {
        self.timeout_seconds.max(1)
    }

    /// Returns validated `max_results`, clamped to `1..=MAX_SEARCH_RESULTS`
    #[must_use]
    pub fn validated_max_results(&self) -> usize {
        self.max_results.clamp(1, MAX_SEARCH_RESULTS)
    }

    // ─── Per-provider parameter mappers ────────────────────────────────
    //
    // These exist as plain methods on SearchOptions (not free functions
    // on each provider) so that the full mapping table stays visible in
    // one file. See the module-level docs for the table.

    /// Brave `freshness` (`pd`/`pw`/`pm`/`py`).
    #[must_use]
    pub fn brave_freshness(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "pd",
            Recency::Week => "pw",
            Recency::Month => "pm",
            Recency::Year => "py",
        })
    }

    /// Brave `safesearch` (`off`/`moderate`).
    #[must_use]
    pub const fn brave_safesearch(&self) -> &'static str {
        if self.safe_search {
            "moderate"
        } else {
            "off"
        }
    }

    /// Bing `freshness` (`Day`/`Week`/`Month`). Bing has no `Year`.
    #[must_use]
    pub fn bing_freshness(&self) -> Option<&'static str> {
        match self.recency? {
            Recency::Day => Some("Day"),
            Recency::Week => Some("Week"),
            Recency::Month => Some("Month"),
            Recency::Year => None,
        }
    }

    /// Bing `safeSearch` (`Off`/`Moderate`).
    #[must_use]
    pub const fn bing_safesearch(&self) -> &'static str {
        if self.safe_search {
            "Moderate"
        } else {
            "Off"
        }
    }

    /// Google CSE `dateRestrict` (`d1`/`w1`/`m1`/`y1`).
    #[must_use]
    pub fn google_date_restrict(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "d1",
            Recency::Week => "w1",
            Recency::Month => "m1",
            Recency::Year => "y1",
        })
    }

    /// Google CSE `safe` (`active`/`off`).
    #[must_use]
    pub const fn google_safe(&self) -> &'static str {
        if self.safe_search {
            "active"
        } else {
            "off"
        }
    }

    /// Google CSE `lr` language restrictor — Google requires the
    /// `lang_` prefix, while SearXNG/Brave/Bing accept the bare code.
    #[must_use]
    pub fn google_lr(&self) -> Option<String> {
        let lang = self.language.as_deref()?;
        Some(format!("lang_{lang}"))
    }

    /// `SearXNG` `time_range` (`day`/`week`/`month`/`year`). Bare token.
    #[must_use]
    pub fn searxng_time_range(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "day",
            Recency::Week => "week",
            Recency::Month => "month",
            Recency::Year => "year",
        })
    }

    /// `SearXNG` `safesearch` (`0`/`1`/`2` for off/moderate/strict).
    /// We expose only Off vs Moderate today; bumping to Strict requires
    /// a new `SearchOptions` field.
    #[must_use]
    pub const fn searxng_safesearch(&self) -> u8 {
        if self.safe_search {
            1
        } else {
            0
        }
    }

    /// Tavily `days` integer (1/7/30/365) — Tavily doesn't take a
    /// freshness token; it takes a "look back N days" int instead.
    #[must_use]
    pub fn tavily_days(&self) -> Option<u32> {
        Some(match self.recency? {
            Recency::Day => 1,
            Recency::Week => 7,
            Recency::Month => 30,
            Recency::Year => 365,
        })
    }

    /// `DuckDuckGo` `kp` (`1`=moderate, `-2`=off; strict is `-1`).
    #[must_use]
    pub const fn ddg_kp(&self) -> &'static str {
        if self.safe_search {
            "1"
        } else {
            "-2"
        }
    }

    /// `DuckDuckGo` `df` (`d`/`w`/`m`/`y`).
    #[must_use]
    pub fn ddg_df(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "d",
            Recency::Week => "w",
            Recency::Month => "m",
            Recency::Year => "y",
        })
    }

    /// Firecrawl `tbs` time filter (Google-style `qdr:d`/`qdr:w`/`qdr:m`/`qdr:y`).
    #[must_use]
    pub fn firecrawl_tbs(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "qdr:d",
            Recency::Week => "qdr:w",
            Recency::Month => "qdr:m",
            Recency::Year => "qdr:y",
        })
    }
}

// ============================================================================
// SearchResult (moved from `src/search/result.rs`)
// ============================================================================

/// Search result entry returned by all providers
///
/// This struct provides a unified interface for search results from
/// different providers (Google, Bing, Tavily, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Result title
    pub title: String,

    /// Source URL
    pub url: String,

    /// Snippet/summary of the content
    pub snippet: String,

    /// Relevance score (0.0 - 1.0)
    /// Tavily provides this natively; others may compute it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,

    /// Full content (only for Tavily deep search)
    /// WARNING: Can be very large, use sparingly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,

    /// When the page says it was published, in whatever spelling the backend
    /// sent (RFC 2822 from Tavily, ISO-8601 from Exa). Not normalised: a
    /// reader can tell 2019 from last week in either, and parsing every
    /// vendor's dialect to re-emit one of them buys nothing a caller asked
    /// for.
    ///
    /// `None` means **this backend did not say** — never "this result has no
    /// date". The distinction is the whole point of the field: a backend that
    /// reports no dates makes every result look equally undated, and a caller
    /// asking about recent work needs to know which of the two they are
    /// looking at. Backends that send nothing leave it absent rather than
    /// having a date invented for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,

    /// Provider that returned this result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchResult {
    /// Create a basic search result (for testing/mocking)
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            relevance_score: None,
            full_content: None,
            published_date: None,
            provider: None,
        }
    }
}

// ============================================================================
// SearchCapabilities + SearchProvider (moved from `src/search/provider.rs`)
// ============================================================================

/// What a provider can express on the wire.
///
/// A bit here is a promise: the registry uses it as a **sorting key** (spec
/// §3) — a provider that claims `domain_filter` gets the requests that ask
/// for one. Claiming a parameter you do not send therefore does not fail
/// loudly, it silently widens somebody's search. `capability_census.rs`
/// compares each bit against the request builder that would have to send it.
///
/// The default is all-`false` on purpose: a new provider that forgets to
/// declare anything is invisible to dimension-aware routing, which is the
/// safe direction. The reverse default would make it claim everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchCapabilities {
    /// Accepts an include/exclude domain list.
    pub domain_filter: bool,
    /// Accepts a freshness constraint (`SearchOptions::recency`).
    pub recency: bool,
    /// Can return page bodies, not just snippets.
    pub full_content: bool,
}

/// Unified interface for search providers
///
/// All search backends (Tavily, Google, `SearXNG`, etc.) implement this trait.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Execute a search query
    ///
    /// # Arguments
    ///
    /// * `query` - Search keywords
    /// * `options` - Search options (language, region, filters, etc.)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<SearchResult>)` - List of search results
    /// * `Err(AlephError)` - Network error, API error, quota exceeded, etc.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aleph_slot_api::search::{SearchProvider, SearchOptions};
    /// # async fn example(provider: &dyn SearchProvider) {
    /// let options = SearchOptions::default();
    /// let results = provider.search("Rust async", &options).await.unwrap();
    /// # }
    /// ```
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>>;

    /// Get provider name (for logging/debugging)
    fn name(&self) -> &str;

    /// Check if provider is configured and available
    ///
    /// Returns `false` if API key is missing or invalid
    fn is_available(&self) -> bool;

    /// What this provider can express **for this request**. Default: nothing
    /// — see [`SearchCapabilities`] for why the default is not "everything".
    ///
    /// Takes the options because a bit is not always a property of the
    /// backend alone: Bing's `freshness` covers three of the four `Recency`
    /// buckets and has no spelling for the fourth, so "does Bing carry
    /// recency" only has an answer once you know *which* recency. Before this
    /// parameter that fact lived in a comment above a hardcoded `true`, and a
    /// `Recency::Year` request was routed to Bing *first* — it declared the
    /// dimension — and then dropped it without a note, which is the exact
    /// failure the notes exist to prevent.
    ///
    /// A conditional bit must be derived from the same mapper that decides
    /// what goes on the wire (`options.bing_freshness().is_some()`), never
    /// re-stated as a second `match`; `capability_census.rs` asserts that.
    fn capabilities(&self, options: &SearchOptions) -> SearchCapabilities {
        let _ = options;
        SearchCapabilities::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── options tests (moved from `src/search/options.rs`) ────────────

    #[test]
    fn test_search_options_default() {
        let options = SearchOptions::default();

        assert_eq!(options.max_results, 5);
        assert_eq!(options.timeout_seconds, 10);
        assert!(options.safe_search);
        assert!(!options.include_full_content);
        assert!(options.language.is_none());
    }

    #[test]
    fn test_search_options_custom_timeout() {
        let options = SearchOptions::default_with_timeout(20);

        assert_eq!(options.timeout_seconds, 20);
        assert_eq!(options.max_results, 5);
    }

    #[test]
    fn test_search_options_customization() {
        let options = SearchOptions {
            language: Some("zh-CN".to_string()),
            region: Some("CN".to_string()),
            recency: Some(Recency::Week),
            safe_search: true,
            max_results: 10,
            timeout_seconds: 15,
            include_full_content: true,
            include_domains: vec![],
            exclude_domains: vec![],
            providers: vec![],
            snippet_budget_chars: None,
        };

        assert_eq!(options.language.unwrap(), "zh-CN");
        assert_eq!(options.max_results, 10);
        assert!(options.include_full_content);
    }

    #[test]
    fn domain_lists_default_to_empty_which_means_no_constraint() {
        let o = SearchOptions::default();
        assert!(o.include_domains.is_empty());
        assert!(o.exclude_domains.is_empty());
    }

    #[test]
    fn test_validated_timeout_bounds() {
        let options = SearchOptions {
            timeout_seconds: 0,
            ..Default::default()
        };
        assert_eq!(options.validated_timeout(), 1);

        let options = SearchOptions {
            timeout_seconds: 5,
            ..Default::default()
        };
        assert_eq!(options.validated_timeout(), 5);
    }

    #[test]
    fn test_validated_max_results_bounds() {
        let options = SearchOptions {
            max_results: 0,
            ..Default::default()
        };
        assert_eq!(options.validated_max_results(), 1);

        let options = SearchOptions {
            max_results: 100,
            ..Default::default()
        };
        assert_eq!(options.validated_max_results(), 100);

        let options = SearchOptions {
            max_results: 500,
            ..Default::default()
        };
        assert_eq!(options.validated_max_results(), MAX_SEARCH_RESULTS);

        let options = SearchOptions {
            max_results: 10,
            ..Default::default()
        };
        assert_eq!(options.validated_max_results(), 10);
    }

    fn opts_with_recency(r: Recency) -> SearchOptions {
        SearchOptions {
            recency: Some(r),
            ..Default::default()
        }
    }

    #[test]
    fn brave_freshness_maps_canonical_tokens() {
        assert_eq!(
            opts_with_recency(Recency::Day).brave_freshness(),
            Some("pd")
        );
        assert_eq!(
            opts_with_recency(Recency::Week).brave_freshness(),
            Some("pw")
        );
        assert_eq!(
            opts_with_recency(Recency::Month).brave_freshness(),
            Some("pm")
        );
        assert_eq!(
            opts_with_recency(Recency::Year).brave_freshness(),
            Some("py")
        );
        assert_eq!(SearchOptions::default().brave_freshness(), None);
    }

    #[test]
    fn bing_freshness_has_no_year() {
        assert_eq!(
            opts_with_recency(Recency::Day).bing_freshness(),
            Some("Day")
        );
        assert_eq!(
            opts_with_recency(Recency::Week).bing_freshness(),
            Some("Week")
        );
        assert_eq!(
            opts_with_recency(Recency::Month).bing_freshness(),
            Some("Month")
        );
        // Bing API has no Year option — must return None, NOT a panic.
        assert_eq!(opts_with_recency(Recency::Year).bing_freshness(), None);
    }

    #[test]
    fn google_date_restrict_uses_n1_suffix() {
        assert_eq!(
            opts_with_recency(Recency::Day).google_date_restrict(),
            Some("d1")
        );
        assert_eq!(
            opts_with_recency(Recency::Week).google_date_restrict(),
            Some("w1")
        );
        assert_eq!(
            opts_with_recency(Recency::Month).google_date_restrict(),
            Some("m1")
        );
        assert_eq!(
            opts_with_recency(Recency::Year).google_date_restrict(),
            Some("y1")
        );
    }

    #[test]
    fn google_lr_prefixes_lang_underscore() {
        let o = SearchOptions {
            language: Some("zh".to_string()),
            ..Default::default()
        };
        assert_eq!(o.google_lr().as_deref(), Some("lang_zh"));
        assert_eq!(SearchOptions::default().google_lr(), None);
    }

    #[test]
    fn tavily_days_converts_range_to_integer() {
        assert_eq!(opts_with_recency(Recency::Day).tavily_days(), Some(1));
        assert_eq!(opts_with_recency(Recency::Week).tavily_days(), Some(7));
        assert_eq!(opts_with_recency(Recency::Month).tavily_days(), Some(30));
        assert_eq!(opts_with_recency(Recency::Year).tavily_days(), Some(365));
    }

    #[test]
    fn safesearch_helpers_toggle_consistently() {
        let on = SearchOptions {
            safe_search: true,
            ..Default::default()
        };
        let off = SearchOptions {
            safe_search: false,
            ..Default::default()
        };
        assert_eq!(on.brave_safesearch(), "moderate");
        assert_eq!(off.brave_safesearch(), "off");
        assert_eq!(on.bing_safesearch(), "Moderate");
        assert_eq!(off.bing_safesearch(), "Off");
        assert_eq!(on.google_safe(), "active");
        assert_eq!(off.google_safe(), "off");
        assert_eq!(on.searxng_safesearch(), 1);
        assert_eq!(off.searxng_safesearch(), 0);
        assert_eq!(on.ddg_kp(), "1");
        assert_eq!(off.ddg_kp(), "-2");
    }

    #[test]
    fn searxng_time_range_passes_through_canonical_tokens() {
        assert_eq!(
            opts_with_recency(Recency::Day).searxng_time_range(),
            Some("day")
        );
        assert_eq!(
            opts_with_recency(Recency::Year).searxng_time_range(),
            Some("year")
        );
    }

    #[test]
    fn ddg_df_uses_single_letter_codes() {
        assert_eq!(opts_with_recency(Recency::Day).ddg_df(), Some("d"));
        assert_eq!(opts_with_recency(Recency::Week).ddg_df(), Some("w"));
        assert_eq!(opts_with_recency(Recency::Month).ddg_df(), Some("m"));
        assert_eq!(opts_with_recency(Recency::Year).ddg_df(), Some("y"));
    }

    #[test]
    fn firecrawl_tbs_maps_canonical_tokens() {
        assert_eq!(
            opts_with_recency(Recency::Day).firecrawl_tbs(),
            Some("qdr:d")
        );
        assert_eq!(
            opts_with_recency(Recency::Week).firecrawl_tbs(),
            Some("qdr:w")
        );
        assert_eq!(
            opts_with_recency(Recency::Month).firecrawl_tbs(),
            Some("qdr:m")
        );
        assert_eq!(
            opts_with_recency(Recency::Year).firecrawl_tbs(),
            Some("qdr:y")
        );
        assert_eq!(SearchOptions::default().firecrawl_tbs(), None);
    }

    #[test]
    fn recency_maps_to_every_provider_vocabulary() {
        use Recency::{Day, Month, Week, Year};
        // (recency, brave, bing, google, searxng, tavily, ddg, firecrawl)
        // Bing has no Year bucket (see bing_freshness_has_no_year above), so
        // its column is `Option<&str>` while the other six are bare `&str`.
        let cases = [
            (Day, "pd", Some("Day"), "d1", "day", 1u32, "d", "qdr:d"),
            (Week, "pw", Some("Week"), "w1", "week", 7, "w", "qdr:w"),
            (Month, "pm", Some("Month"), "m1", "month", 30, "m", "qdr:m"),
            (Year, "py", None, "y1", "year", 365, "y", "qdr:y"),
        ];
        for (r, brave, bing, google, searxng, tavily, ddg, firecrawl) in cases {
            let o = SearchOptions {
                recency: Some(r),
                ..Default::default()
            };
            assert_eq!(o.brave_freshness(), Some(brave), "{r:?}");
            assert_eq!(o.bing_freshness(), bing, "{r:?}");
            assert_eq!(o.google_date_restrict(), Some(google), "{r:?}");
            assert_eq!(o.searxng_time_range(), Some(searxng), "{r:?}");
            assert_eq!(o.tavily_days(), Some(tavily), "{r:?}");
            assert_eq!(o.ddg_df(), Some(ddg), "{r:?}");
            assert_eq!(o.firecrawl_tbs(), Some(firecrawl), "{r:?}");
        }
    }

    /// The whole point of the enum: a value outside the four-word table is
    /// rejected at the edge instead of being dropped by seven mappers, each of
    /// which used to answer `None` and let the caller believe it had constrained
    /// the search.
    #[test]
    fn an_unknown_recency_string_is_rejected_not_dropped() {
        let err = serde_json::from_value::<Recency>(serde_json::json!("7d")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("day"),
            "the error must list the legal values: {msg}"
        );
    }

    #[test]
    fn no_recency_means_no_provider_parameter() {
        let o = SearchOptions::default();
        assert_eq!(o.brave_freshness(), None);
        assert_eq!(o.tavily_days(), None);
        assert_eq!(o.firecrawl_tbs(), None);
    }

    // ─── result tests (moved from `src/search/result.rs`) ──────────────

    #[test]
    fn test_search_result_new() {
        let result = SearchResult::new(
            "Test Title".to_string(),
            "https://example.com".to_string(),
            "Test snippet".to_string(),
        );

        assert_eq!(result.title, "Test Title");
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.snippet, "Test snippet");
        assert!(result.provider.is_none());
    }

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult::new(
            "Test".to_string(),
            "https://test.com".to_string(),
            "Snippet".to_string(),
        );

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result, deserialized);
    }

    /// A field this struct once carried and no longer does (`source_type`)
    /// must still parse — we don't `deny_unknown_fields`, and that is the
    /// wire-level back-compat this test pins.
    ///
    /// `published_date` is back, so it is no longer one of them: it was cut in
    /// `8297b1fce` for having zero producers, and returns here with two
    /// (Tavily and Exa) and a renderer on the tool face. It is typed as the
    /// string the vendors send; the numeric epoch this test used to pass was
    /// never a shape anything emitted, since every provider left the field
    /// `None` and `skip_serializing_if` meant it never reached the wire at
    /// all.
    #[test]
    fn test_search_result_ignores_legacy_fields() {
        let legacy_json = r#"{
            "title": "Test",
            "url": "https://test.com",
            "snippet": "Snippet",
            "published_date": "Mon, 01 Jan 2024 00:00:00 GMT",
            "source_type": "article",
            "relevance_score": 0.5,
            "provider": "tavily"
        }"#;

        let parsed: SearchResult = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.relevance_score, Some(0.5));
        assert_eq!(parsed.provider.as_deref(), Some("tavily"));
        assert_eq!(
            parsed.published_date.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 GMT")
        );
    }

    #[test]
    fn test_search_result_with_optional_fields() {
        let result = SearchResult {
            title: "Test".to_string(),
            url: "https://test.com".to_string(),
            snippet: "Test snippet".to_string(),
            relevance_score: Some(0.95),
            full_content: None,
            published_date: None,
            provider: Some("tavily".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"title\":\"Test\""));
        assert!(json.contains("\"relevance_score\":0.95"));

        let parsed: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.relevance_score, Some(0.95));
    }

    // ─── provider tests (moved from `src/search/provider.rs`) ──────────

    #[test]
    fn test_provider_is_send_sync() {
        // This test ensures SearchProvider can be used across threads
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<std::sync::Arc<dyn SearchProvider>>();
    }
}

/// Search options configuration
///
/// This module defines options passed to search providers, plus the
/// per-provider parameter mappings for the four "rich" fields
/// (`language`, `region`, `recency`, `safe_search`). Each provider
/// expects different field names and value vocabularies for the same
/// concept — by collecting the mappings here, the per-provider HTTP
/// builders stay simple (each just calls one helper) and adding a new
/// provider requires extending this mapping table rather than reverse-
/// engineering the convention from a sibling provider.
///
/// Mapping table (kept in sync with the helpers below):
///
/// | field        | Brave         | Bing         | Google CSE   | `SearXNG`      | Tavily   | `DuckDuckGo` | Firecrawl |
/// |--------------|---------------|--------------|--------------|--------------|----------|------------|-----------|
/// | language     | `search_lang`   | setLang      | `lr=lang_XX`   | language     | —        | —          | lang      |
/// | region       | country       | cc           | gl           | —            | —        | kl         | country   |
/// | `recency`      | freshness     | freshness    | dateRestrict | `time_range`   | days     | df         | tbs       |
/// | `safe_search`  | safesearch    | safeSearch   | safe         | safesearch   | —        | kp         | —         |
///
/// Providers that have no native concept for a field omit it entirely
/// (the helper returns `None` or the call site simply doesn't push it).
use serde::{Deserialize, Serialize};

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
}

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

    /// Returns validated `max_results`, capped at 50 and at least 1
    #[must_use]
    pub fn validated_max_results(&self) -> usize {
        self.max_results.clamp(1, 50)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        assert_eq!(options.language.unwrap(), "zh-CN");
        assert_eq!(options.max_results, 10);
        assert!(options.include_full_content);
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
        assert_eq!(options.validated_max_results(), 50);

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
}

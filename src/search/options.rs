/// Search options configuration
///
/// This module defines options passed to search providers, plus the
/// per-provider parameter mappings for the four "rich" fields
/// (`language`, `region`, `date_range`, `safe_search`). Each provider
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
/// | `date_range`   | freshness     | freshness    | dateRestrict | `time_range`   | days     | df         | tbs       |
/// | `safe_search`  | safesearch    | safeSearch   | safe         | safesearch   | —        | kp         | —         |
///
/// Providers that have no native concept for a field omit it entirely
/// (the helper returns `None` or the call site simply doesn't push it).
use serde::{Deserialize, Serialize};

/// Search options passed to providers.
///
/// See module-level docs for the per-provider mapping table covering
/// `language`/`region`/`date_range`/`safe_search`.
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

    /// Date range filter — one of "day" / "week" / "month" / "year".
    /// Other values are silently ignored by the per-provider mappers.
    /// Forwarded to Brave/Bing/Google/SearXNG/Tavily/DDG; ignored elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<String>,

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
            date_range: None,
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
        Some(match self.date_range.as_deref()? {
            "day" => "pd",
            "week" => "pw",
            "month" => "pm",
            "year" => "py",
            _ => return None,
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
        Some(match self.date_range.as_deref()? {
            "day" => "Day",
            "week" => "Week",
            "month" => "Month",
            _ => return None,
        })
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
        Some(match self.date_range.as_deref()? {
            "day" => "d1",
            "week" => "w1",
            "month" => "m1",
            "year" => "y1",
            _ => return None,
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
        Some(match self.date_range.as_deref()? {
            "day" => "day",
            "week" => "week",
            "month" => "month",
            "year" => "year",
            _ => return None,
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
        Some(match self.date_range.as_deref()? {
            "day" => 1,
            "week" => 7,
            "month" => 30,
            "year" => 365,
            _ => return None,
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
        Some(match self.date_range.as_deref()? {
            "day" => "d",
            "week" => "w",
            "month" => "m",
            "year" => "y",
            _ => return None,
        })
    }

    /// Firecrawl `tbs` time filter (Google-style `qdr:d`/`qdr:w`/`qdr:m`/`qdr:y`).
    #[must_use]
    pub fn firecrawl_tbs(&self) -> Option<&'static str> {
        Some(match self.date_range.as_deref()? {
            "day" => "qdr:d",
            "week" => "qdr:w",
            "month" => "qdr:m",
            "year" => "qdr:y",
            _ => return None,
        })
    }
}

/// Quota information for rate-limited providers
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaInfo {
    /// Remaining searches in current period
    pub remaining: Option<u32>,

    /// Total quota limit
    pub limit: Option<u32>,

    /// Reset timestamp (Unix timestamp)
    pub reset_at: Option<i64>,
}

impl QuotaInfo {
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            remaining: None,
            limit: None,
            reset_at: None,
        }
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
            date_range: Some("week".to_string()),
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
    fn test_quota_info_unlimited() {
        let quota = QuotaInfo::unlimited();

        assert!(quota.remaining.is_none());
        assert!(quota.limit.is_none());
        assert!(quota.reset_at.is_none());
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

    fn opts_with_range(d: &str) -> SearchOptions {
        SearchOptions {
            date_range: Some(d.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn brave_freshness_maps_canonical_tokens() {
        assert_eq!(opts_with_range("day").brave_freshness(), Some("pd"));
        assert_eq!(opts_with_range("week").brave_freshness(), Some("pw"));
        assert_eq!(opts_with_range("month").brave_freshness(), Some("pm"));
        assert_eq!(opts_with_range("year").brave_freshness(), Some("py"));
        assert_eq!(opts_with_range("garbage").brave_freshness(), None);
        assert_eq!(SearchOptions::default().brave_freshness(), None);
    }

    #[test]
    fn bing_freshness_has_no_year() {
        assert_eq!(opts_with_range("day").bing_freshness(), Some("Day"));
        assert_eq!(opts_with_range("week").bing_freshness(), Some("Week"));
        assert_eq!(opts_with_range("month").bing_freshness(), Some("Month"));
        // Bing API has no Year option — must return None, NOT a panic.
        assert_eq!(opts_with_range("year").bing_freshness(), None);
    }

    #[test]
    fn google_date_restrict_uses_n1_suffix() {
        assert_eq!(opts_with_range("day").google_date_restrict(), Some("d1"));
        assert_eq!(opts_with_range("week").google_date_restrict(), Some("w1"));
        assert_eq!(opts_with_range("month").google_date_restrict(), Some("m1"));
        assert_eq!(opts_with_range("year").google_date_restrict(), Some("y1"));
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
        assert_eq!(opts_with_range("day").tavily_days(), Some(1));
        assert_eq!(opts_with_range("week").tavily_days(), Some(7));
        assert_eq!(opts_with_range("month").tavily_days(), Some(30));
        assert_eq!(opts_with_range("year").tavily_days(), Some(365));
        assert_eq!(opts_with_range("decade").tavily_days(), None);
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
        assert_eq!(opts_with_range("day").searxng_time_range(), Some("day"));
        assert_eq!(opts_with_range("year").searxng_time_range(), Some("year"));
        assert_eq!(opts_with_range("century").searxng_time_range(), None);
    }

    #[test]
    fn ddg_df_uses_single_letter_codes() {
        assert_eq!(opts_with_range("day").ddg_df(), Some("d"));
        assert_eq!(opts_with_range("week").ddg_df(), Some("w"));
        assert_eq!(opts_with_range("month").ddg_df(), Some("m"));
        assert_eq!(opts_with_range("year").ddg_df(), Some("y"));
    }

    #[test]
    fn firecrawl_tbs_maps_canonical_tokens() {
        assert_eq!(opts_with_range("day").firecrawl_tbs(), Some("qdr:d"));
        assert_eq!(opts_with_range("week").firecrawl_tbs(), Some("qdr:w"));
        assert_eq!(opts_with_range("month").firecrawl_tbs(), Some("qdr:m"));
        assert_eq!(opts_with_range("year").firecrawl_tbs(), Some("qdr:y"));
        assert_eq!(opts_with_range("garbage").firecrawl_tbs(), None);
        assert_eq!(SearchOptions::default().firecrawl_tbs(), None);
    }
}

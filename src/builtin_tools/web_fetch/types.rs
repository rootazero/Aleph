//! Public types for the web fetch tool.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Content extraction mode
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExtractMode {
    /// Structured Markdown output (default)
    #[default]
    Markdown,
    /// Plain text output (legacy behavior)
    Text,
}

/// Which extraction method produced the content
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Extractor {
    /// Mozilla Readability algorithm
    Readability,
    /// CSS selector-based fallback
    Selector,
    /// crawl4ai backend (operator-hosted headless crawler → markdown)
    Crawl4ai,
    /// Firecrawl backend (operator-hosted structured extraction API)
    Firecrawl,
}

impl Extractor {
    /// Map a `FetchProvider::name()` value to the corresponding [`Extractor`].
    ///
    /// Used by `web_fetch` to record which backend actually served the
    /// content — previously the result hardcoded [`Self::Crawl4ai`]
    /// regardless of whether crawl4ai, firecrawl, or any future provider
    /// had done the work, which is form-5 name drift in `WebFetchResult`.
    /// Unknown providers fall through to [`Self::Crawl4ai`] so legacy
    /// callers (and the read-only built-in fallback path that doesn't go
    /// through `FetchProvider`) keep working; new providers are expected
    /// to land here before being added to the registry.
    #[must_use]
    pub fn for_provider_name(name: &str) -> Self {
        match name {
            "firecrawl" => Self::Firecrawl,
            // crawl4ai is the default; covers the legacy hardcoded path
            // and any operator-hosted headless crawler whose name we
            // haven't enumerated yet.
            _ => Self::Crawl4ai,
        }
    }

    /// Stable lower-case token used in the JSON wire format and in the
    /// human-facing result summary. Matches the `rename_all = "lowercase"`
    /// serde tag so callers can compare against the raw JSON field
    /// without going through `serde_json::to_value` first.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Readability => "readability",
            Self::Selector => "selector",
            Self::Crawl4ai => "crawl4ai",
            Self::Firecrawl => "firecrawl",
        }
    }
}

/// Arguments for web fetch tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
    /// Content extraction mode (default: markdown)
    #[serde(default)]
    pub extract_mode: ExtractMode,
    /// Optional natural-language focus for the fetch.
    ///
    /// When set, the prompt is prepended to the returned content as a
    /// `[fetch_focus: ...]` marker, telling the main agent loop what
    /// to look for inside the page. This is intentionally NOT a
    /// secondary-LLM extraction step (the claude-code approach):
    ///
    /// * The main agent loop will read this tool's output anyway —
    ///   forcing a second LLM hop adds latency and cost on every
    ///   fetch without adding reasoning the main model couldn't do.
    /// * Aleph's R9 principle ("intelligence lives in the prompt")
    ///   prefers steering the existing LLM via context over running
    ///   an extra model. R10 ("thin harness") rules out the
    ///   provider plumbing this would otherwise require.
    ///
    /// If a downstream consumer ever needs an actual condensed
    /// summary, the right place to add it is at the agent layer, not
    /// inside the fetch tool — keep tools dumb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// Web fetch result containing extracted content
#[derive(Debug, Clone, Serialize)]
pub struct WebFetchResult {
    /// The fetched URL
    pub url: String,
    /// Page title extracted from <title> tag
    pub title: Option<String>,
    /// Main text content extracted from the page
    pub content: String,
    /// Which extraction method was used
    pub extractor: Extractor,
}

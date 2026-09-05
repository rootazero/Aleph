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
///
/// Only the two built-in extractors remain: the crawl4ai/firecrawl variants
/// were removed when fetch-provider delegation was unwired (BT-D-R4-22 — the
/// SSRF DNS pin cannot be enforced on a provider-side crawl, so `web_fetch`
/// no longer routes to external URL→markdown backends).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Extractor {
    /// Mozilla Readability algorithm
    Readability,
    /// CSS selector-based fallback
    Selector,
    /// PDF text-layer extraction via lopdf (per-page `[page N]` markers)
    Pdf,
    /// YouTube transcript via yt-dlp (subtitles, VTT cleaned)
    Youtube,
}

impl Extractor {
    /// Stable lower-case token used in the JSON wire format and in the
    /// human-facing result summary. Matches the `rename_all = "lowercase"`
    /// serde tag so callers can compare against the raw JSON field
    /// without going through `serde_json::to_value` first.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Readability => "readability",
            Self::Selector => "selector",
            Self::Pdf => "pdf",
            Self::Youtube => "youtube",
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

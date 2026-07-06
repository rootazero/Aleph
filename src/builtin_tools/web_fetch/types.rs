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
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Extractor {
    /// Mozilla Readability algorithm
    Readability,
    /// CSS selector-based fallback
    Selector,
    /// crawl4ai backend (operator-hosted headless crawler → markdown)
    Crawl4ai,
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

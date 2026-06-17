//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements `AlephTool` trait for AI agent integration.

use super::error::ToolError;
use crate::config::WebFetchPolicy;
use crate::error::Result;
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::sync_primitives::Mutex;
use crate::tools::AlephTool;
use async_trait::async_trait;
use lru::LruCache;
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};
use tracing::{debug, info};

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
}

// ---------------------------------------------------------------------------
// Pre-compiled regexes for HTML cleaning (compiled once, reused forever)
// ---------------------------------------------------------------------------

static RE_COMMENTS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));
static RE_SCRIPT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?si)<script(\s[^>]*)?>.*?</script\s*>").expect("valid regex"));
static RE_STYLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?si)<style(\s[^>]*)?>.*?</style\s*>").expect("valid regex"));
static RE_NOSCRIPT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?si)<noscript(\s[^>]*)?>.*?</noscript\s*>").expect("valid regex"));
static RE_HIDDEN_ATTR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?si)<[^>]+\shidden(\s[^>]*)?>.*?</[^>]+>").expect("valid regex"));
static RE_ARIA_HIDDEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?si)<[^>]+\saria-hidden\s*=\s*["']true["'][^>]*>.*?</[^>]+>"#)
        .expect("valid regex")
});
static RE_DISPLAY_NONE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?si)<[^>]+\sstyle\s*=\s*["'][^"']*(?:display\s*:\s*none|visibility\s*:\s*hidden)[^"']*["'][^>]*>.*?</[^>]+>"#,
    )
    .expect("valid regex")
});
static RE_SR_CLASSES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?si)<[^>]+\sclass\s*=\s*["'][^"']*(?:sr-only|visually-hidden|d-none|screen-reader-only)[^"']*["'][^>]*>.*?</[^>]+>"#,
    )
    .expect("valid regex")
});
static RE_STRIP_TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").expect("valid regex"));

// ---------------------------------------------------------------------------
// URL fetch cache (inspired by claude-code's WebFetchTool LRU)
// ---------------------------------------------------------------------------
//
// Aleph-server is a long-running daemon; the same URL is frequently re-asked
// across a single agent loop (e.g. an LLM that re-reads a doc page in 3
// different sub-steps). Caching the parsed result avoids hammering the
// upstream + paying repeat extract cost. Sized by entry count (not bytes)
// for simplicity — each entry's body is already capped at ~10 KB after
// markdown extraction, so 256 entries is < 3 MB worst case.
//
// Key is (canonical-URL, extract_mode) because the same URL fetched as
// Markdown vs Text yields different content.
//
// Invalidation is purely TTL-based (15 min); we don't honour HTTP
// Cache-Control because most LLM-driven re-fetches are within seconds of
// each other and a 15-min ceiling is the right blast radius.

const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const CACHE_CAPACITY: usize = 256;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CacheKey {
    /// Canonical URL: lowercased scheme+host, default port stripped,
    /// fragment removed. Path and query preserved verbatim.
    url: String,
    extract_mode: ExtractModeKey,
}

/// Discriminant-only copy of `ExtractMode` so it can be cheaply used as
/// part of the cache key without coupling to its serde-aware definition.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum ExtractModeKey {
    Markdown,
    Text,
}

impl From<&ExtractMode> for ExtractModeKey {
    fn from(m: &ExtractMode) -> Self {
        match m {
            ExtractMode::Markdown => Self::Markdown,
            ExtractMode::Text => Self::Text,
        }
    }
}

struct CacheEntry {
    result: WebFetchResult,
    inserted_at: Instant,
}

static URL_CACHE: Lazy<Mutex<LruCache<CacheKey, CacheEntry>>> = Lazy::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(CACHE_CAPACITY).expect("CACHE_CAPACITY > 0"),
    ))
});

/// Best-effort URL canonicalisation. Falls back to the raw URL if `url`
/// can't parse it (e.g. caller already sent something the SSRF layer
/// will reject anyway). Lowercasing the host + dropping fragment +
/// default-port-stripping covers >95% of "same URL different string"
/// cases without inviting more aggressive normalisation bugs.
fn canonicalize_url(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    parsed.set_fragment(None);
    // `url` lowercases scheme and host on parse already; default ports
    // are normalised by calling set_port(None) when the port equals
    // the scheme's default.
    if matches!(
        (parsed.scheme(), parsed.port()),
        ("http", Some(80)) | ("https", Some(443)) | ("ws", Some(80)) | ("wss", Some(443))
    ) {
        let _ = parsed.set_port(None);
    }
    parsed.to_string()
}

fn cache_key(url: &str, mode: &ExtractMode) -> CacheKey {
    CacheKey {
        url: canonicalize_url(url),
        extract_mode: ExtractModeKey::from(mode),
    }
}

fn cache_lookup(key: &CacheKey) -> Option<WebFetchResult> {
    let mut guard = URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    // `LruCache::get` mutates recency, so we need &mut.
    let entry = guard.get(key)?;
    if entry.inserted_at.elapsed() > CACHE_TTL {
        guard.pop(key);
        return None;
    }
    Some(entry.result.clone())
}

fn cache_store(key: CacheKey, result: WebFetchResult) {
    let mut guard = URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    guard.put(
        key,
        CacheEntry {
            result,
            inserted_at: Instant::now(),
        },
    );
}

#[cfg(test)]
fn cache_clear() {
    URL_CACHE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Prepend a `[fetch_focus: ...]` marker to the result's content when
/// the caller supplied a non-empty prompt. The marker sits OUTSIDE the
/// content-boundary wrap because it comes from the (trusted) LLM tool
/// call, not from the (untrusted) fetched page.
///
/// Long prompts are clipped at 512 chars and newlines are flattened to
/// spaces — the marker is meant to be a one-liner steering hint, not a
/// multi-paragraph spec.
fn apply_focus_prompt(mut result: WebFetchResult, prompt: Option<&str>) -> WebFetchResult {
    let Some(p) = prompt.map(str::trim).filter(|s| !s.is_empty()) else {
        return result;
    };
    let mut marker = String::with_capacity(p.len() + 32);
    marker.push_str("[fetch_focus: ");
    for ch in p.chars().take(512) {
        if ch == '\n' || ch == '\r' {
            marker.push(' ');
        } else {
            marker.push(ch);
        }
    }
    marker.push_str("]\n\n");
    marker.push_str(&result.content);
    result.content = marker;
    result
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

/// Web fetch tool for retrieving and extracting content from web pages
pub struct WebFetchTool {
    /// Maximum content length in characters (from policy)
    max_content_length: usize,
    /// Minimum content length to accept a selector match (from policy)
    min_content_length: usize,
    /// User agent string (from policy)
    user_agent: String,
    /// Request timeout in seconds
    timeout_secs: u64,
    /// Whether Readability extraction is enabled
    enable_readability: bool,
    /// SSRF protection policy
    ssrf_policy: SsrfPolicy,
}

impl WebFetchTool {
    /// Tool name constant
    pub const NAME: &'static str = "web_fetch";

    /// Tool description for AI
    pub const DESCRIPTION: &'static str = "Fetch and extract text content from a web page URL.";

    /// Default maximum content length (used when no policy provided)
    const DEFAULT_MAX_CONTENT_LENGTH: usize = 10000;

    /// Default minimum content length (used when no policy provided)
    const DEFAULT_MIN_CONTENT_LENGTH: usize = 100;

    /// Default user agent string (used when no policy provided)
    const DEFAULT_USER_AGENT: &'static str = "Aleph/1.0";

    /// Default request timeout in seconds (used when no policy provided)
    const DEFAULT_TIMEOUT_SECS: u64 = 30;

    /// Maximum response body size in bytes (10 MB)
    const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

    /// Create a new `WebFetchTool` with default settings
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_content_length: Self::DEFAULT_MAX_CONTENT_LENGTH,
            min_content_length: Self::DEFAULT_MIN_CONTENT_LENGTH,
            user_agent: Self::DEFAULT_USER_AGENT.to_string(),
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
            enable_readability: true,
            ssrf_policy: SsrfPolicy::default(),
        }
    }

    /// Set the SSRF policy
    #[must_use]
    pub fn with_ssrf_policy(mut self, policy: SsrfPolicy) -> Self {
        self.ssrf_policy = policy;
        self
    }

    /// Create a new `WebFetchTool` with policy configuration
    #[must_use]
    pub fn with_policy(policy: &WebFetchPolicy) -> Self {
        Self {
            max_content_length: policy.max_content_length as usize,
            min_content_length: policy.min_content_length as usize,
            user_agent: policy.user_agent.clone(),
            timeout_secs: policy.timeout_seconds,
            enable_readability: policy.enable_readability,
            ssrf_policy: SsrfPolicy::default(),
        }
    }

    /// Fetch and extract content from a URL (internal implementation)
    async fn call_impl(
        &self,
        args: WebFetchArgs,
    ) -> std::result::Result<WebFetchResult, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Notify tool start
        let url_display = crate::utils::text_format::truncate_text(&args.url, 50);
        notify_tool_start(Self::NAME, &format!("获取网页: {url_display}"));

        // Cache lookup BEFORE notify_tool_start would otherwise be cleaner
        // semantically, but we want the "fetching ..." progress notice to
        // appear even on cache hits so the operator can still trace which
        // URL was requested. The cached path then immediately notifies
        // success with a "(cached)" marker.
        //
        // Note: the cache key intentionally does NOT include `args.prompt`
        // — the focus marker is prepended on the way out (here, and on
        // cache miss). This means two calls to the same URL with
        // different prompts share the same cached page body, which is
        // the right cost/freshness tradeoff for LLM-driven re-fetches.
        let key = cache_key(&args.url, &args.extract_mode);
        if let Some(cached) = cache_lookup(&key) {
            debug!("web_fetch cache hit: {}", args.url);
            let result = apply_focus_prompt(cached, args.prompt.as_deref());
            let summary = format!("已获取网页内容 ({} 字符, cached)", result.content.len());
            notify_tool_result(Self::NAME, &summary, true);
            return Ok(result);
        }

        info!("Fetching URL: {}", args.url);

        // SSRF-protected fetch with DNS pinning
        let ssrf_policy = &self.ssrf_policy;
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(ua) = reqwest::header::HeaderValue::from_str(&self.user_agent) {
            headers.insert(reqwest::header::USER_AGENT, ua);
        }
        let fetch_request =
            SafeFetchRequest::get(std::time::Duration::from_secs(self.timeout_secs))
                .with_headers(headers);

        let fetch_response = safe_fetch(&args.url, ssrf_policy, fetch_request)
            .await
            .map_err(|e| {
                let error_msg = format!("Fetch blocked or failed: {e}");
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Network(error_msg)
            })?;

        if !fetch_response.status.is_success() {
            let error_msg = format!(
                "HTTP error: {} for URL: {}",
                fetch_response.status, args.url
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Network(error_msg));
        }

        let bytes = &fetch_response.body;

        if bytes.len() > Self::MAX_RESPONSE_BYTES {
            let error_msg = format!(
                "Response too large: {} bytes (max {} bytes)",
                bytes.len(),
                Self::MAX_RESPONSE_BYTES,
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Execution(error_msg));
        }

        let html_content = String::from_utf8_lossy(bytes).to_string();

        debug!("Fetched {} bytes from {}", html_content.len(), args.url);

        // Safety gate: reject oversized HTML
        Self::validate_html_safety(&html_content).inspect_err(|e| {
            notify_tool_result(Self::NAME, &e.to_string(), false);
        })?;

        // Extract title from raw HTML (before pre-cleaning)
        let document = Html::parse_document(&html_content);
        let title = self.extract_title(&document);
        debug!("Extracted title: {:?}", title);

        // Enhanced extraction: Readability + Markdown with selector fallback
        let (content, extractor) =
            self.extract_content_enhanced(&html_content, &args.url, &args.extract_mode);
        debug!(
            "Extracted {} chars via {:?} extractor",
            content.len(),
            extractor
        );

        // Notify success
        let extractor_name = match &extractor {
            Extractor::Readability => "readability",
            Extractor::Selector => "selector",
        };
        let result_summary = format!(
            "已获取网页内容 ({} 字符, {})",
            content.len(),
            extractor_name,
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        // Wrap with external content boundary markers
        let wrapped_content = wrap_external_content(
            &content,
            ContentSource::WebFetch {
                url: args.url.clone(),
            },
        );

        // Cache the BARE wrapped result (no focus prompt) so subsequent
        // fetches with different prompts can share the cached body.
        let bare_result = WebFetchResult {
            url: args.url,
            title,
            content: wrapped_content,
            extractor,
        };
        cache_store(key, bare_result.clone());
        Ok(apply_focus_prompt(bare_result, args.prompt.as_deref()))
    }

    /// Extract the page title from <title> tag
    fn extract_title(&self, document: &Html) -> Option<String> {
        let selector = Selector::parse("title").ok()?;
        document
            .select(&selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Extract main content using priority-ordered selectors
    fn extract_content(&self, document: &Html) -> String {
        // Content selectors in priority order
        let selectors = [
            "article",
            "main",
            ".content",
            ".post-content",
            "#content",
            "body",
        ];

        for selector_str in selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                let content = document
                    .select(&selector)
                    .next()
                    .map(|el| self.clean_text(&el.text().collect::<String>()))
                    .unwrap_or_default();

                if content.len() > self.min_content_length {
                    debug!(
                        "Using selector '{}' with {} chars",
                        selector_str,
                        content.len()
                    );
                    return self.truncate_content(content);
                }
            }
        }

        // Fallback: return whatever we can get from body
        if let Ok(selector) = Selector::parse("body") {
            let content = document
                .select(&selector)
                .next()
                .map(|el| self.clean_text(&el.text().collect::<String>()))
                .unwrap_or_default();
            return self.truncate_content(content);
        }

        String::new()
    }

    /// Clean whitespace from text (collapse multiple spaces)
    fn clean_text(&self, text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Truncate content to maximum length
    fn truncate_content(&self, content: String) -> String {
        if content.chars().count() <= self.max_content_length {
            content
        } else {
            // Truncate at character boundary
            let truncated: String = content.chars().take(self.max_content_length).collect();
            format!("{truncated}...")
        }
    }

    // -----------------------------------------------------------------------
    // Part A: Pre-cleaning and safety gates
    // -----------------------------------------------------------------------

    /// Reject HTML that exceeds the 10 MB response budget to prevent `DoS`.
    pub(crate) fn validate_html_safety(html: &str) -> std::result::Result<(), ToolError> {
        // The 10 MB response-byte gate already bounds anything reaching here;
        // this is the matching upper bound on the decoded HTML string. The
        // former 1 MB cap rejected ordinary modern news pages (1–4 MB of HTML)
        // *before* the readability/markdown extractor could reduce them to
        // clean, length-capped text — defeating the whole point of fetching.
        if html.len() > Self::MAX_RESPONSE_BYTES {
            return Err(ToolError::Execution(format!(
                "HTML too large: {} bytes (max {} bytes)",
                html.len(),
                Self::MAX_RESPONSE_BYTES,
            )));
        }
        Ok(())
    }

    /// Remove noise from raw HTML before extraction:
    /// zero-width Unicode, HTML comments, script/style/noscript blocks,
    /// and elements that are visually hidden.
    pub(crate) fn pre_clean_html(html: &str) -> String {
        // 1. Strip zero-width / invisible Unicode characters
        let zero_width: &[char] = &[
            '\u{200B}', // ZERO WIDTH SPACE
            '\u{200C}', // ZERO WIDTH NON-JOINER
            '\u{200D}', // ZERO WIDTH JOINER
            '\u{200E}', // LEFT-TO-RIGHT MARK
            '\u{200F}', // RIGHT-TO-LEFT MARK
            '\u{FEFF}', // ZERO WIDTH NO-BREAK SPACE (BOM)
            '\u{2060}', // WORD JOINER
        ];
        let cleaned: String = html.chars().filter(|c| !zero_width.contains(c)).collect();

        // 2. Remove HTML comments
        let cleaned = RE_COMMENTS.replace_all(&cleaned, "").to_string();

        // 3. Remove <script>, <style>, <noscript> blocks with their content.
        let cleaned = RE_SCRIPT.replace_all(&cleaned, "").to_string();
        let cleaned = RE_STYLE.replace_all(&cleaned, "").to_string();
        let cleaned = RE_NOSCRIPT.replace_all(&cleaned, "").to_string();

        // 4. Remove elements with hidden attribute
        let cleaned = RE_HIDDEN_ATTR.replace_all(&cleaned, "").to_string();

        // 5. Remove elements with aria-hidden="true"
        let cleaned = RE_ARIA_HIDDEN.replace_all(&cleaned, "").to_string();

        // 6. Remove elements with display:none or visibility:hidden in style attribute
        let cleaned = RE_DISPLAY_NONE.replace_all(&cleaned, "").to_string();

        // 7. Remove elements with screen-reader / visually-hidden CSS classes
        RE_SR_CLASSES.replace_all(&cleaned, "").to_string()
    }

    // -----------------------------------------------------------------------
    // Part B: Readability + Markdown pipeline
    // -----------------------------------------------------------------------

    /// Run the Mozilla Readability algorithm on the HTML and return clean
    /// article HTML. Returns `None` when the result is empty or too short.
    fn extract_with_readability(&self, html: &str, url: &str) -> Option<String> {
        use url::Url;

        let parsed_url = Url::parse(url).ok()?;
        let mut cursor = std::io::Cursor::new(html.as_bytes());
        let product = readability::extractor::extract(&mut cursor, &parsed_url).ok()?;

        if product.content.len() < self.min_content_length {
            return None;
        }
        Some(product.content)
    }

    /// Convert an HTML string to Markdown using `htmd`. Falls back to
    /// tag-stripping on conversion failure.
    fn html_to_markdown(&self, html: &str) -> String {
        match htmd::convert(html) {
            Ok(md) => md,
            Err(_) => self.strip_tags(html),
        }
    }

    /// Strip HTML tags from a string using a simple regex, leaving plain text.
    fn strip_tags(&self, html: &str) -> String {
        let text = RE_STRIP_TAGS.replace_all(html, " ").to_string();
        self.clean_text(&text)
    }

    /// Enhanced extraction pipeline: pre-clean → Readability → Markdown/Text.
    /// Falls back to the legacy selector-based extractor when Readability
    /// fails or the result is too short.
    pub(crate) fn extract_content_enhanced(
        &self,
        raw_html: &str,
        url: &str,
        mode: &ExtractMode,
    ) -> (String, Extractor) {
        let cleaned_html = Self::pre_clean_html(raw_html);

        // Try Readability extraction (if enabled)
        if self.enable_readability {
            if let Some(article_html) = self.extract_with_readability(&cleaned_html, url) {
                let content = match mode {
                    ExtractMode::Markdown => self.html_to_markdown(&article_html),
                    ExtractMode::Text => {
                        let text = self.strip_tags(&article_html);
                        self.clean_text(&text)
                    }
                };
                let content = self.truncate_content(content);
                return (content, Extractor::Readability);
            }
        }

        // Fallback: legacy CSS-selector-based extraction
        let document = Html::parse_document(raw_html);
        let content = self.extract_content(&document);
        (content, Extractor::Selector)
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WebFetchTool {
    fn clone(&self) -> Self {
        Self {
            max_content_length: self.max_content_length,
            min_content_length: self.min_content_length,
            user_agent: self.user_agent.clone(),
            timeout_secs: self.timeout_secs,
            enable_readability: self.enable_readability,
            ssrf_policy: self.ssrf_policy.clone(),
        }
    }
}

/// Implementation of `AlephTool` trait for `WebFetchTool`
#[async_trait]
impl AlephTool for WebFetchTool {
    const NAME: &'static str = "web_fetch";
    const DESCRIPTION: &'static str = "Fetch and extract text content from a web page URL.";

    type Args = WebFetchArgs;
    type Output = WebFetchResult;

    /// A fetched page is reliably larger than the global default; cap at 10k
    /// tokens (was the legacy `resolve_result_budget` name-table value for
    /// `web_fetch`).
    fn max_result_tokens(&self) -> Option<usize> {
        Some(10_000)
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::ssrf::{validate_url, SsrfPolicy};
    use crate::tools::AlephTool;
    // Note: validate_url is still used by SSRF unit tests below (test_ssrf_*)

    /// Serialises tests that touch the process-global `URL_CACHE`. They each
    /// call `cache_clear()`, so without this guard a parallel sweep lets one
    /// test wipe another's just-stored entry, producing intermittent failures.
    /// Uses `std::sync::Mutex` (not the crate alias) so the `const` initialiser
    /// holds regardless of the `loom` feature.
    static CACHE_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_web_fetch_args() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert_eq!(args.url, "https://example.com");
    }

    #[test]
    fn test_web_fetch_tool_creation() {
        let tool = WebFetchTool::new();
        assert_eq!(WebFetchTool::NAME, "web_fetch");
        assert!(!WebFetchTool::DESCRIPTION.is_empty());
        // Verify the tool was created successfully
        drop(tool);
    }

    #[tokio::test]
    #[ignore] // Requires network connection
    async fn test_web_fetch_call() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs {
            url: "https://example.com".to_string(),
            extract_mode: ExtractMode::Markdown,
            prompt: None,
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        let result = result.unwrap();
        assert_eq!(result.url, "https://example.com");
        assert!(result.title.is_some(), "Expected title to be present");
        assert!(
            result.title.as_ref().unwrap().contains("Example"),
            "Expected title to contain 'Example'"
        );
        assert!(!result.content.is_empty(), "Expected content to be present");
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs {
            url: "not-a-valid-url".to_string(),
            extract_mode: ExtractMode::Markdown,
            prompt: None,
        };

        // Use fully qualified syntax to avoid ambiguity
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err(), "Expected error for invalid URL");

        // Error is now AlephError wrapping the SSRF/fetch error
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("Fetch blocked or failed"),
            "Expected 'Fetch blocked or failed' error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_clean_text() {
        let tool = WebFetchTool::new();
        let text = "  Hello   world  \n\t  test  ";
        let cleaned = tool.clean_text(text);
        assert_eq!(cleaned, "Hello world test");
    }

    #[test]
    fn test_ssrf_blocks_localhost() {
        let policy = SsrfPolicy::default();
        assert!(validate_url("http://localhost/admin", &policy).is_err());
        assert!(validate_url("http://127.0.0.1/secret", &policy).is_err());
        assert!(validate_url("http://127.0.0.1:8080/api", &policy).is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_networks() {
        let policy = SsrfPolicy::default();
        assert!(validate_url("http://10.0.0.1/internal", &policy).is_err());
        assert!(validate_url("http://192.168.1.1/admin", &policy).is_err());
        assert!(validate_url("http://172.16.0.1/secret", &policy).is_err());
    }

    #[test]
    fn test_ssrf_blocks_metadata_endpoints() {
        let policy = SsrfPolicy::default();
        assert!(validate_url("http://169.254.169.254/latest/meta-data/", &policy).is_err());
        assert!(validate_url("http://metadata.google.internal/computeMetadata/", &policy).is_err());
    }

    #[test]
    fn test_ssrf_allows_public_urls() {
        let policy = SsrfPolicy::default();
        assert!(validate_url("https://example.com", &policy).is_ok());
        assert!(validate_url("https://8.8.8.8", &policy).is_ok());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_loopback() {
        let policy = SsrfPolicy::default();
        assert!(validate_url("http://[::1]/admin", &policy).is_err());
    }

    #[test]
    fn test_truncate_content() {
        let tool = WebFetchTool::new();

        // Short content should not be truncated
        let short = "Hello world".to_string();
        assert_eq!(tool.truncate_content(short.clone()), short);

        // Long content should be truncated
        let long = "a".repeat(15000);
        let truncated = tool.truncate_content(long);
        assert!(truncated.len() <= WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH + 3); // +3 for "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_extract_mode_defaults_to_markdown() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert!(matches!(args.extract_mode, ExtractMode::Markdown));
    }

    #[test]
    fn test_extract_mode_text() {
        let args: WebFetchArgs =
            serde_json::from_str(r#"{"url": "https://example.com", "extract_mode": "text"}"#)
                .unwrap();
        assert!(matches!(args.extract_mode, ExtractMode::Text));
    }

    #[test]
    fn test_pre_clean_removes_script_and_style() {
        let html = r#"<html><body>
            <script>alert('xss')</script>
            <style>.hide { display: none; }</style>
            <p>Visible content here</p>
        </body></html>"#;
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(
            !cleaned.contains("alert"),
            "Script content should be removed: {}",
            cleaned
        );
        assert!(
            !cleaned.contains(".hide"),
            "Style content should be removed: {}",
            cleaned
        );
        assert!(
            cleaned.contains("Visible content here"),
            "Visible text should remain: {}",
            cleaned
        );
    }

    #[test]
    fn test_pre_clean_removes_hidden_elements() {
        let html = r#"<html><body>
            <div style="display:none">Hidden div</div>
            <div aria-hidden="true">Aria hidden div</div>
            <p>Visible paragraph</p>
        </body></html>"#;
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(
            !cleaned.contains("Hidden div"),
            "display:none should be removed: {}",
            cleaned
        );
        assert!(
            !cleaned.contains("Aria hidden div"),
            "aria-hidden should be removed: {}",
            cleaned
        );
        assert!(
            cleaned.contains("Visible paragraph"),
            "Visible text should remain: {}",
            cleaned
        );
    }

    #[test]
    fn test_pre_clean_strips_zero_width_chars() {
        let html = "<html><body><p>Hello\u{200B}World\u{FEFF}Test</p></body></html>";
        let cleaned = WebFetchTool::pre_clean_html(html);
        assert!(
            cleaned.contains("HelloWorldTest"),
            "Zero-width chars should be stripped: {}",
            cleaned
        );
    }

    #[test]
    fn test_safety_gate_rejects_oversized_html() {
        // Only truly pathological input (beyond the 10 MB response budget) is
        // rejected; the byte gate already bounds anything that reaches here.
        let huge = "a".repeat(WebFetchTool::MAX_RESPONSE_BYTES + 1);
        assert!(WebFetchTool::validate_html_safety(&huge).is_err());
    }

    #[test]
    fn test_safety_gate_accepts_large_news_page() {
        // Real news section pages routinely ship 1–4 MB of HTML (e.g. BBC
        // Middle East ≈ 3.6 MB). These must pass the gate so the readability
        // extractor can reduce them to clean text — the old 1 MB cap rejected
        // them outright before extraction.
        let big = "a".repeat(3_600_000);
        assert!(WebFetchTool::validate_html_safety(&big).is_ok());
    }

    #[test]
    fn test_safety_gate_accepts_normal_html() {
        let normal = "<html><body><p>Hello</p></body></html>";
        assert!(WebFetchTool::validate_html_safety(normal).is_ok());
    }

    #[test]
    fn test_readability_extraction_produces_markdown() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test Article</title></head>
        <body>
            <nav><a href="/">Home</a> | <a href="/about">About</a> | <a href="/contact">Contact</a></nav>
            <article>
                <h1>Main Article Title</h1>
                <p>This is the first paragraph of the article with enough content to be recognized by the readability algorithm as meaningful text content that should be extracted and preserved in the output.</p>
                <p>This is the second paragraph providing additional detail about the topic being discussed in this article. It contains several sentences to ensure adequate length for proper extraction.</p>
                <h2>Section Two</h2>
                <ul>
                    <li>First item in the list</li>
                    <li>Second item in the list</li>
                    <li>Third item in the list</li>
                </ul>
                <p>A concluding paragraph that wraps up the discussion and provides final thoughts on the matter at hand. This ensures the article has sufficient content density.</p>
            </article>
            <footer><p>Copyright 2024 | Privacy Policy | Terms of Service</p></footer>
        </body></html>"#;

        let tool = WebFetchTool::new();
        let (content, _extractor) = tool.extract_content_enhanced(
            html,
            "https://example.com/article",
            &ExtractMode::Markdown,
        );

        // Should produce non-empty content
        assert!(!content.is_empty(), "Content should not be empty");
        // Should contain article text
        assert!(
            content.contains("Main Article Title") || content.contains("first paragraph"),
            "Should contain article content: {}",
            &content[..content.len().min(500)]
        );
    }

    #[test]
    fn test_text_mode_produces_plain_text() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title Here</h1>
            <p>This is a paragraph with enough content for readability to extract it properly as meaningful text content in the article body section.</p>
            <p>Another paragraph with sufficient length to ensure the readability algorithm recognizes this as article content worth preserving in output.</p>
        </article></body></html>"#;

        let tool = WebFetchTool::new();
        let (content, _) =
            tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Text);

        // Should contain the actual text
        assert!(
            content.contains("paragraph") || content.contains("Title"),
            "Should contain article text: {}",
            content
        );
    }

    #[test]
    fn test_fallback_to_selector_on_minimal_html() {
        let html = "<html><body><p>Short</p></body></html>";
        let tool = WebFetchTool::new();
        let (_, extractor) =
            tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Markdown);

        assert!(
            matches!(extractor, Extractor::Selector),
            "Expected Selector fallback, got {:?}",
            extractor
        );
    }

    #[test]
    fn test_readability_disabled_uses_selector() {
        let html = r#"<!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body><article>
            <h1>Title</h1>
            <p>Long enough paragraph for readability to normally extract this content from the article body with sufficient detail and words.</p>
            <p>Another paragraph ensuring adequate length for the readability algorithm to process and recognize as valid content.</p>
        </article></body></html>"#;

        let mut tool = WebFetchTool::new();
        tool.enable_readability = false;
        let (_, extractor) =
            tool.extract_content_enhanced(html, "https://example.com", &ExtractMode::Markdown);

        assert!(
            matches!(extractor, Extractor::Selector),
            "Should use Selector when readability disabled, got {:?}",
            extractor
        );
    }

    #[test]
    fn test_extractor_serialization() {
        let result = WebFetchResult {
            url: "https://example.com".to_string(),
            title: Some("Test".to_string()),
            content: "# Hello".to_string(),
            extractor: Extractor::Readability,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["extractor"], "readability");
    }

    // ─── URL cache ─────────────────────────────────────────────────────

    fn dummy_result(url: &str, content: &str) -> WebFetchResult {
        WebFetchResult {
            url: url.to_string(),
            title: None,
            content: content.to_string(),
            extractor: Extractor::Selector,
        }
    }

    #[test]
    fn canonicalize_url_strips_default_port_and_fragment() {
        assert_eq!(
            canonicalize_url("HTTPS://Example.COM:443/path?q=1#frag"),
            "https://example.com/path?q=1"
        );
        assert_eq!(
            canonicalize_url("http://example.com:80/"),
            "http://example.com/"
        );
        assert_eq!(
            canonicalize_url("https://example.com:8443/"),
            "https://example.com:8443/"
        );
        // Junk URLs pass through unchanged so the SSRF layer can reject them.
        assert_eq!(canonicalize_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn cache_key_distinguishes_extract_modes() {
        let k1 = cache_key("https://example.com/", &ExtractMode::Markdown);
        let k2 = cache_key("https://example.com/", &ExtractMode::Text);
        assert_ne!(
            k1, k2,
            "same URL with different extract modes must be separate cache entries"
        );
    }

    #[test]
    fn cache_lookup_returns_stored_entry() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        cache_clear();
        let key = cache_key("https://cache-test.invalid/a", &ExtractMode::Markdown);
        assert!(cache_lookup(&key).is_none(), "fresh cache should miss");

        cache_store(
            key.clone(),
            dummy_result("https://cache-test.invalid/a", "hi"),
        );
        let got = cache_lookup(&key).expect("should hit");
        assert_eq!(got.content, "hi");
    }

    #[test]
    fn cache_lookup_returns_none_for_expired_entry() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        cache_clear();
        let key = cache_key("https://cache-test.invalid/b", &ExtractMode::Markdown);
        // Direct insert with an `inserted_at` in the past — bypass
        // `cache_store` so the test doesn't have to actually wait 15
        // minutes for the TTL to elapse.
        {
            let mut guard = URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            guard.put(
                key.clone(),
                CacheEntry {
                    result: dummy_result("https://cache-test.invalid/b", "stale"),
                    inserted_at: Instant::now()
                        .checked_sub(CACHE_TTL + Duration::from_secs(1))
                        .expect("Instant arithmetic"),
                },
            );
        }
        assert!(
            cache_lookup(&key).is_none(),
            "expired entry must be reported as a miss"
        );
        // And evicted.
        let guard = URL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(guard.peek(&key).is_none(), "expired entry must be evicted");
    }

    #[test]
    fn cache_key_normalises_url_for_hit() {
        let _guard = CACHE_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        cache_clear();
        let stored = cache_key("HTTPS://Example.com:443/path", &ExtractMode::Markdown);
        cache_store(stored, dummy_result("https://example.com/path", "ok"));

        // Caller requests the same URL with a different surface form —
        // canonicalisation should bring them to the same cache slot.
        let looked = cache_key("https://example.com/path#frag", &ExtractMode::Markdown);
        assert!(
            cache_lookup(&looked).is_some(),
            "URLs differing only in case/port/fragment must share a cache entry"
        );
    }

    // ─── Focus prompt ──────────────────────────────────────────────────

    #[test]
    fn args_accept_prompt_field_with_back_compat_default() {
        // Pre-existing TOML/JSON without the prompt key must still parse.
        let bare: WebFetchArgs = serde_json::from_str(r#"{"url": "https://x.test/"}"#).unwrap();
        assert_eq!(bare.prompt, None);

        let with_prompt: WebFetchArgs = serde_json::from_str(
            r#"{"url": "https://x.test/", "prompt": "find the pricing table"}"#,
        )
        .unwrap();
        assert_eq!(
            with_prompt.prompt.as_deref(),
            Some("find the pricing table")
        );
    }

    #[test]
    fn apply_focus_prompt_prepends_marker() {
        let original = dummy_result("https://x.test/", "PAGE BODY");
        let with_focus = apply_focus_prompt(original, Some("show pricing"));
        assert!(
            with_focus
                .content
                .starts_with("[fetch_focus: show pricing]\n\n"),
            "marker not prepended; got: {:?}",
            with_focus.content
        );
        assert!(with_focus.content.ends_with("PAGE BODY"));
    }

    #[test]
    fn apply_focus_prompt_is_noop_for_none_or_blank() {
        let original = dummy_result("https://x.test/", "PAGE");
        assert_eq!(apply_focus_prompt(original.clone(), None).content, "PAGE",);
        assert_eq!(
            apply_focus_prompt(original.clone(), Some("   ")).content,
            "PAGE",
            "whitespace-only prompts should not produce a marker"
        );
        assert_eq!(apply_focus_prompt(original, Some("")).content, "PAGE");
    }

    #[test]
    fn apply_focus_prompt_flattens_newlines_and_clips_length() {
        let original = dummy_result("https://x.test/", "BODY");
        let long_multiline = format!("part one\npart two\r\n{}", "x".repeat(600));
        let out = apply_focus_prompt(original, Some(&long_multiline));
        // Marker is 1 line — no embedded \n inside the [fetch_focus: ...] segment.
        let marker_end = out
            .content
            .find("]\n\n")
            .expect("marker terminator should be present");
        let marker_text = &out.content[..marker_end];
        assert!(!marker_text[1..].contains('\n'));
        // Clipped at 512 chars of prompt content.
        let prompt_text = &marker_text["[fetch_focus: ".len()..];
        assert!(prompt_text.chars().count() <= 512);
    }
}

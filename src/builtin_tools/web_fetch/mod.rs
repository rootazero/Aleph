//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements `AlephTool` trait for AI agent integration.

mod cache;
mod extract;
mod pdf;
mod types;

// YouTube transcript path. Dispatched in `call_impl` before the HTTP
// fetch; gated by `[policies.web_fetch] youtube_transcript` (default on).
mod youtube;

pub use types::{ExtractMode, Extractor, WebFetchArgs, WebFetchResult};

use super::error::ToolError;
use crate::config::WebFetchPolicy;
use crate::error::Result;
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::tools::AlephTool;
use async_trait::async_trait;
use scraper::Html;
use tracing::{debug, info};

use cache::{cache_key, cache_lookup, cache_store};

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
    /// Whether PDF responses take the lopdf text-extraction pipeline
    pdf_extract: bool,
    /// Whether YouTube URLs take the yt-dlp transcript pipeline
    youtube_transcript: bool,
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

    /// Fetch-time body cap. Raised to the PDF budget (20 MB) so PDFs can
    /// use their full budget regardless of how they were dispatched
    /// (Content-Type or URL hint); non-PDF responses are still rejected
    /// past 10 MB by the post-fetch check below, so HTML behavior is
    /// unchanged. The streamed cap only moves the point at which a
    /// hostile >10 MB HTML page aborts from "during download" to
    /// "after download" — the rejection itself is identical.
    const FETCH_BODY_CAP: usize = pdf::MAX_PDF_BYTES;

    /// Create a new `WebFetchTool` with default settings
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_content_length: Self::DEFAULT_MAX_CONTENT_LENGTH,
            min_content_length: Self::DEFAULT_MIN_CONTENT_LENGTH,
            user_agent: Self::DEFAULT_USER_AGENT.to_string(),
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
            enable_readability: true,
            pdf_extract: true,
            youtube_transcript: true,
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
            pdf_extract: policy.pdf_extract,
            youtube_transcript: policy.youtube_transcript,
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

        // YouTube special case: the transcript comes from yt-dlp, not from
        // fetching the watch page (whose HTML carries almost no readable
        // text). SSRF is safe by construction here: `detect_youtube` only
        // matches real YouTube hosts and `fetch_transcript` re-derives a
        // canonical youtube.com URL from the bare video id, so no
        // caller-controlled host reaches the network. Soft failures (yt-dlp
        // not installed, video has no subtitles) fall through to the generic
        // HTTP path; hard failures are honest errors.
        if self.youtube_transcript {
            if let Some(target) = youtube::detect_youtube(&args.url) {
                match youtube::fetch_transcript(&target).await {
                    Ok(transcript) => {
                        debug!(
                            "YouTube transcript: {} chars for {}",
                            transcript.text().len(),
                            args.url
                        );
                        return Ok(self.finalize_success(
                            args,
                            key,
                            None,
                            transcript.text(),
                            Extractor::Youtube,
                        ));
                    }
                    Err(e) if e.is_soft() => {
                        info!(
                            "YouTube transcript unavailable for {} ({e}); falling back to HTTP fetch",
                            args.url
                        );
                    }
                    Err(e) => {
                        let error_msg = format!("YouTube transcript failed: {e}");
                        notify_tool_result(Self::NAME, &error_msg, false);
                        return Err(ToolError::Execution(error_msg));
                    }
                }
            }
        }

        // No fetch-provider branch here. `[fetch]` providers (crawl4ai,
        // firecrawl) are deliberately NOT wired into this tool: they receive
        // the target URL as a string and resolve/follow it on their own
        // network, so the SSRF DNS pin computed here cannot be enforced on
        // the fetch that actually happens (BT-D-R4-22). Neither provider API
        // accepts a pre-resolved address, and routing only the Aleph→provider
        // hop through `safe_fetch` would leave the audited High-severity gap
        // (provider-side rebinding + redirect following inside the LAN) wide
        // open. The constructor logs a one-time startup warning when `[fetch]`
        // is configured so the config surface is not silently inert.
        info!("Fetching URL: {}", args.url);

        // SSRF-protected fetch with DNS pinning
        let ssrf_policy = &self.ssrf_policy;
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(ua) = reqwest::header::HeaderValue::from_str(&self.user_agent) {
            headers.insert(reqwest::header::USER_AGENT, ua);
        }
        let fetch_request =
            SafeFetchRequest::get(std::time::Duration::from_secs(self.timeout_secs))
                .with_headers(headers)
                .with_max_body_bytes(Self::FETCH_BODY_CAP);

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

        // PDF special case, dispatched BEFORE the HTML size gate: PDFs
        // carry a 20 MB byte budget, HTML pages keep 10 MB. When the
        // policy switch is off, PDFs fall through to the legacy HTML
        // path unchanged.
        if self.pdf_extract && pdf::is_pdf_response(&fetch_response.headers, &args.url) {
            return self.handle_pdf(bytes, args, key);
        }

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

        Ok(self.finalize_success(args, key, title, &content, extractor))
    }

    /// PDF branch of `call_impl`: extract the text layer with lopdf, then
    /// run the exact same post-processing as the HTML path. Failures are
    /// honest errors — never a fallback to parsing binary as HTML.
    fn handle_pdf(
        &self,
        bytes: &[u8],
        args: WebFetchArgs,
        key: cache::CacheKey,
    ) -> std::result::Result<WebFetchResult, ToolError> {
        use super::notify_tool_result;

        debug!("PDF response detected for {}", args.url);
        let (title, text) = pdf::extract_pdf(bytes).inspect_err(|e| {
            notify_tool_result(Self::NAME, &e.to_string(), false);
        })?;
        debug!(
            "Extracted {} chars of PDF text from {}",
            text.len(),
            args.url
        );
        Ok(self.finalize_success(args, key, title, &text, Extractor::Pdf))
    }

    /// Shared success tail for the HTML and PDF paths: notify, cap the
    /// sanitized image, wrap with external-content boundary markers,
    /// cache the bare result, then apply the focus-prompt marker.
    fn finalize_success(
        &self,
        args: WebFetchArgs,
        key: cache::CacheKey,
        title: Option<String>,
        content: &str,
        extractor: Extractor,
    ) -> WebFetchResult {
        use super::notify_tool_result;

        let WebFetchArgs { url, prompt, .. } = args;

        let result_summary = format!(
            "已获取网页内容 ({} 字符, {})",
            content.len(),
            extractor.as_str(),
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        // Wrap with external content boundary markers. The content arrives
        // raw-capped from extraction; `truncate_fetched` re-caps the
        // SANITIZED image so placeholder growth (a 3-char `<s>` becomes a
        // 23-char `[REMOVED_SPECIAL_TOKEN]`) cannot push the fenced payload
        // past `max_content_length`.
        let content = self.truncate_fetched(content);
        let wrapped_content =
            wrap_external_content(&content, ContentSource::WebFetch { url: url.clone() });

        // Cache the BARE wrapped result (no focus prompt) so subsequent
        // fetches with different prompts can share the cached body.
        let bare_result = WebFetchResult {
            url,
            title,
            content: wrapped_content,
            extractor,
        };
        cache_store(key, bare_result.clone());
        apply_focus_prompt(bare_result, prompt.as_deref())
    }

    /// Extract the page title from <title> tag
    fn extract_title(&self, document: &Html) -> Option<String> {
        extract::extract_title(document)
    }

    /// Reject HTML that exceeds the 10 MB response budget to prevent `DoS`.
    pub(crate) fn validate_html_safety(html: &str) -> std::result::Result<(), ToolError> {
        extract::validate_html_safety(html, Self::MAX_RESPONSE_BYTES)
    }

    /// Truncate content to maximum length
    /// Cap fetched content at `max_content_length` chars of SANITIZED text.
    ///
    /// The cap applies to what the model actually reads: sanitization inside
    /// [`wrap_external_content`] can grow the string (tokenizer markers
    /// become 23-char placeholders), so truncating raw text to the cap first
    /// would still let the fenced payload exceed it — and a raw cut can land
    /// inside a forged boundary marker, leaving a stub the sanitizer cannot
    /// see. `truncate_sanitized_external_content` solves both; the "..."
    /// suffix convention from the old raw truncation is preserved so
    /// downstream consumers still see the truncation signal.
    fn truncate_fetched(&self, content: &str) -> String {
        let t = crate::security::content_sanitizer::truncate_sanitized_external_content(
            content,
            self.max_content_length,
        );
        if t.truncated {
            format!("{}...", t.text)
        } else {
            t.text
        }
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
        extract::extract_content_enhanced(
            raw_html,
            url,
            mode,
            self.enable_readability,
            self.min_content_length,
            self.max_content_length,
        )
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
            pdf_extract: self.pdf_extract,
            youtube_transcript: self.youtube_transcript,
            ssrf_policy: self.ssrf_policy.clone(),
        }
    }
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

/// Implementation of `AlephTool` trait for `WebFetchTool`
#[async_trait]
impl AlephTool for WebFetchTool {
    const NAME: &'static str = "web_fetch";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

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
    use crate::tools::AlephTool;

    fn dummy_result(url: &str, content: &str) -> WebFetchResult {
        WebFetchResult {
            url: url.to_string(),
            title: None,
            content: content.to_string(),
            extractor: Extractor::Selector,
        }
    }

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
    fn test_truncate_fetched() {
        let tool = WebFetchTool::new();

        // Short content should not be truncated
        let short = "Hello world".to_string();
        assert_eq!(tool.truncate_fetched(&short), short);

        // Long content should be truncated
        let long = "a".repeat(15000);
        let truncated = tool.truncate_fetched(&long);
        assert!(truncated.chars().count() <= WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH + 3); // +3 for "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn fetched_truncation_caps_the_sanitized_image() {
        // `<s>` is 3 raw chars but sanitizes to a 23-char placeholder. The
        // cap must absorb that growth, not be defeated by it.
        let tool = WebFetchTool::new();
        let hostile = "<s>".repeat(4000); // 12_000 raw chars → far over cap sanitized
        let out = tool.truncate_fetched(&hostile);
        assert!(
            out.chars().count() <= WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH + 3,
            "sanitized image exceeded cap: {} chars",
            out.chars().count()
        );
        assert!(!out.contains("<s>"), "raw marker survived: {:.80}", out);
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

    #[test]
    fn pdf_extractor_serializes_as_pdf() {
        assert_eq!(serde_json::to_value(Extractor::Pdf).unwrap(), "pdf");
        assert_eq!(Extractor::Pdf.as_str(), "pdf");
    }

    #[test]
    fn with_policy_maps_pdf_extract_flag() {
        let default_tool = WebFetchTool::new();
        assert!(default_tool.pdf_extract);

        let policy = WebFetchPolicy {
            pdf_extract: false,
            ..WebFetchPolicy::default()
        };
        let tool = WebFetchTool::with_policy(&policy);
        assert!(!tool.pdf_extract);
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

    /// The built-in path is the ONLY path: with no provider wiring left
    /// (BT-D-R4-22 removal), the SSRF gate inside `safe_fetch` must refuse
    /// the cloud metadata endpoint outright. An IP literal keeps the block
    /// decision pre-DNS, so the test is hermetic.
    #[tokio::test]
    async fn ssrf_gate_blocks_metadata_endpoint() {
        let tool = WebFetchTool::new();
        let result = tool
            .call_impl(WebFetchArgs {
                url: "http://169.254.169.254/latest/meta-data/".to_string(),
                extract_mode: ExtractMode::Markdown,
                prompt: None,
            })
            .await;
        assert!(
            result.is_err(),
            "metadata endpoint must be refused, got: {result:?}"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Fetch blocked or failed"),
            "expected SSRF refusal, got: {msg}"
        );
    }
}

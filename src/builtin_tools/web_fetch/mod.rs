//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements `AlephTool` trait for AI agent integration.

mod cache;
mod extract;
mod types;

pub use types::{ExtractMode, Extractor, WebFetchArgs, WebFetchResult};

use super::error::ToolError;
use crate::config::WebFetchPolicy;
use crate::error::Result;
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::security::ssrf::{safe_fetch, validate_url_async, SafeFetchRequest, SsrfPolicy};
use crate::tools::AlephTool;
use async_trait::async_trait;
use scraper::Html;
use tracing::{debug, info, warn};

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
    /// SSRF protection policy
    ssrf_policy: SsrfPolicy,
    /// Configured fetch providers (from `[fetch]`), tried in order before the
    /// built-in path. Empty → built-in reqwest+readability only.
    fetch_providers: Vec<std::sync::Arc<dyn crate::fetch::FetchProvider>>,
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
            fetch_providers: Vec::new(),
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
            fetch_providers: Vec::new(),
        }
    }

    /// Inject the selected fetch providers (from `[fetch]`). Empty = built-in only.
    #[must_use]
    pub fn with_fetch_providers(
        mut self,
        providers: Vec<std::sync::Arc<dyn crate::fetch::FetchProvider>>,
    ) -> Self {
        self.fetch_providers = providers;
        self
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

        // Configured fetch providers (if any): URL → markdown via an operator-
        // hosted backend. SSRF-validate the *target* URL once so the agent can't
        // use a provider to reach internal hosts. On any provider failure, fall
        // through to the next provider, then the built-in fetch below.
        //
        // This is NOT redundant with the `safe_fetch` further down: that call is
        // only reached when there are no providers or every provider failed. A
        // provider is a confused deputy — `crawl4ai` is operator-hosted (its own
        // config example is a LAN address), so it dereferences the URL from
        // inside the network the SSRF policy exists to protect, using its own
        // HTTP client that we do not control.
        if !self.fetch_providers.is_empty() {
            // BT-D-R4-22: validate_url_async returns (Url, SocketAddr)
            // where the SocketAddr is the DNS pin we use for the built-in
            // reqwest path. The fetch-provider path here does not (yet)
            // thread that pin into the provider — the provider resolves DNS
            // again inside its own HTTP client, so a provider fetch is
            // exposed to DNS-rebinding in the gap between this validate and
            // the provider's connection. The validate still gates on host
            // policy (block list / private-range) so a provider cannot
            // reach a denied host outright, but it cannot guarantee the
            // exact IP the provider will connect to. Log the gap so
            // operators see it, until the provider API learns to accept a
            // pre-resolved pin.
            match validate_url_async(&args.url, &self.ssrf_policy).await {
                Ok((_url, _pinned)) => {
                    warn!(
                        url = %args.url,
                        "fetch provider path does not thread the validated DNS pin; \
                         provider performs its own resolution (SSRF host-policy gate enforced, \
                         IP-pin TOCTOU window remains)"
                    );
                }
                Err(e) => {
                    let msg = format!("Fetch blocked or failed: {e}");
                    notify_tool_result(Self::NAME, &msg, false);
                    return Err(ToolError::Network(msg));
                }
            }
            for provider in &self.fetch_providers {
                match provider.fetch(&args.url).await {
                    Ok(markdown) => {
                        let content = self.truncate_fetched(&markdown);
                        let summary = format!(
                            "已获取网页内容 ({} 字符, {})",
                            content.len(),
                            provider.name()
                        );
                        notify_tool_result(Self::NAME, &summary, true);
                        let wrapped = wrap_external_content(
                            &content,
                            ContentSource::WebFetch {
                                url: args.url.clone(),
                            },
                        );
                        let bare = WebFetchResult {
                            url: args.url.clone(),
                            title: None,
                            content: wrapped,
                            // Records the backend that actually served the
                            // content; previously this was hardcoded to
                            // `Crawl4ai`, which silently lied when firecrawl
                            // (or any future provider) had done the work
                            // (form-5 name drift in the result envelope).
                            extractor: Extractor::for_provider_name(provider.name()),
                        };
                        cache_store(key, bare.clone());
                        return Ok(apply_focus_prompt(bare, args.prompt.as_deref()));
                    }
                    Err(e) => {
                        warn!(
                            "fetch provider '{}' failed, trying next: {e}",
                            provider.name()
                        );
                    }
                }
            }
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
                .with_headers(headers)
                .with_max_body_bytes(Self::MAX_RESPONSE_BYTES);

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
        let extractor_name = extractor.as_str();
        let result_summary = format!(
            "已获取网页内容 ({} 字符, {})",
            content.len(),
            extractor_name,
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        // Wrap with external content boundary markers. The content arrives
        // raw-capped from extraction; `truncate_fetched` re-caps the
        // SANITIZED image so placeholder growth (a 3-char `<s>` becomes a
        // 23-char `[REMOVED_SPECIAL_TOKEN]`) cannot push the fenced payload
        // past `max_content_length`.
        let content = self.truncate_fetched(&content);
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
            ssrf_policy: self.ssrf_policy.clone(),
            fetch_providers: self.fetch_providers.clone(),
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
    use crate::security::ssrf::SsrfPolicy;
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
    fn extractor_crawl4ai_serializes_to_lowercase() {
        let result = WebFetchResult {
            url: "https://example.com".to_string(),
            title: None,
            content: "# Hello".to_string(),
            extractor: Extractor::Crawl4ai,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["extractor"], "crawl4ai");
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

    #[test]
    fn new_tool_has_no_fetch_providers() {
        let tool = WebFetchTool::new();
        assert!(
            tool.fetch_providers.is_empty(),
            "default tool must have no fetch providers"
        );
    }

    #[tokio::test]
    async fn uses_fetch_provider_before_builtin() {
        struct TestProvider;
        #[async_trait::async_trait]
        impl crate::fetch::FetchProvider for TestProvider {
            async fn fetch(&self, _url: &str) -> crate::error::Result<String> {
                Ok("# FROM-PROVIDER\n\nbody".into())
            }
            fn name(&self) -> &str {
                "test"
            }
            fn is_available(&self) -> bool {
                true
            }
        }
        let tool = WebFetchTool::new()
            .with_ssrf_policy(SsrfPolicy::disabled())
            .with_fetch_providers(vec![std::sync::Arc::new(TestProvider)]);
        let result = tool
            .call_impl(WebFetchArgs {
                url: "https://example.com/fetch-provider-test".to_string(),
                extract_mode: ExtractMode::Markdown,
                prompt: None,
            })
            .await
            .unwrap();
        assert!(
            result.content.contains("FROM-PROVIDER"),
            "expected provider content in result; got: {:?}",
            &result.content[..result.content.len().min(200)]
        );
    }

    /// A configured fetch provider is a confused deputy: `crawl4ai` is an
    /// operator-hosted service (its own config example is `http://10.0.0.1:11235`
    /// — a LAN address), so handing it an attacker-chosen URL makes *it* reach
    /// the internal host on the agent's behalf. The built-in `safe_fetch` below
    /// never runs on this path, so the target URL must be validated before the
    /// provider is handed anything.
    #[tokio::test]
    async fn fetch_provider_path_still_validates_ssrf() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        struct SpyProvider(Arc<AtomicBool>);
        #[async_trait::async_trait]
        impl crate::fetch::FetchProvider for SpyProvider {
            async fn fetch(&self, _url: &str) -> crate::error::Result<String> {
                self.0.store(true, Ordering::SeqCst);
                Ok("# LEAKED-INTERNAL-CONTENT".into())
            }
            fn name(&self) -> &str {
                "spy"
            }
            fn is_available(&self) -> bool {
                true
            }
        }

        let called = Arc::new(AtomicBool::new(false));
        let tool =
            WebFetchTool::new().with_fetch_providers(vec![Arc::new(SpyProvider(called.clone()))]);

        let result = tool
            .call_impl(WebFetchArgs {
                // Cloud metadata endpoint: an IP literal, so the block decision
                // needs no DNS and the test stays hermetic.
                url: "http://169.254.169.254/latest/meta-data/".to_string(),
                extract_mode: ExtractMode::Markdown,
                prompt: None,
            })
            .await;

        assert!(
            result.is_err(),
            "metadata endpoint must be refused, got: {result:?}"
        );
        assert!(
            !called.load(Ordering::SeqCst),
            "provider must never be handed a blocked URL — it fetches from \
             inside the operator's network"
        );
    }
}

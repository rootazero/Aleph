//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements `AlephTool` trait for AI agent integration.

mod cache;
mod extract;
mod intent;
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

    /// How much of a page is extracted when the call carries a `prompt`: the
    /// whole page is indexed for fetch-by-intent (see `intent`), so it is not
    /// cut at the result's content cap first. Bounds the extraction of a
    /// pathological page; the response itself is already capped at 10 MB.
    const INTENT_EXTRACT_MAX_CHARS: usize = 2_000_000;

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
        // A call with a `prompt` needs the whole page (fetch by intent), and the
        // cache holds the capped one — so it fetches.
        let cached = if focus_of(args.prompt.as_deref()).is_some() {
            None
        } else {
            cache_lookup(&key)
        };
        if let Some(cached) = cached {
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
        let cap = if focus_of(args.prompt.as_deref()).is_some() {
            Self::INTENT_EXTRACT_MAX_CHARS
        } else {
            self.content_cap(Self::result_budget_tokens())
        };
        let (content, extractor) =
            self.extract_content_enhanced(&html_content, &args.url, &args.extract_mode, cap);
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

        // Every result is sized to the budget Layer 2 holds it to, so it comes
        // back inline instead of being offloaded again behind a marker.
        let budget = Self::result_budget_tokens();
        let cap = self.content_cap(budget);

        // Fetch by intent: a page too large to return whole comes back as the
        // sections matching the prompt, plus the handle to the rest. Not cached
        // — the cache holds whole-page results, and this one is per prompt.
        if let Some(focus) = focus_of(prompt.as_deref()) {
            if content.chars().count() > cap {
                if let Some((store, call_id)) = Self::intent_store() {
                    // Indexed once; only the selection is refit below, so the
                    // page's blob is written exactly once.
                    if let Some(footer) = intent::index_page(&store, &call_id, &url, content) {
                        let result =
                            fit_to_budget(budget, cap, Some(focus), |cap| WebFetchResult {
                                url: url.clone(),
                                title: title.clone(),
                                content: intent::sections_for_prompt(
                                    &store, &call_id, &url, focus, content, &footer, cap,
                                ),
                                extractor: extractor.clone(),
                            });
                        return apply_focus_prompt(result, Some(focus));
                    }
                }
            }
        }

        // Wrap with external content boundary markers. The content arrives
        // raw-capped from extraction; `truncate_fetched` re-caps the
        // SANITIZED image so placeholder growth (a 3-char `<s>` becomes a
        // 23-char `[REMOVED_SPECIAL_TOKEN]`) cannot push the fenced payload
        // past the cap.
        //
        // Cache the BARE wrapped result (no focus prompt) so subsequent
        // fetches with different prompts can share the cached body.
        let bare_result = fit_to_budget(budget, cap, prompt.as_deref(), |cap| WebFetchResult {
            url: url.clone(),
            title: title.clone(),
            content: wrap_external_content(
                &Self::truncate_fetched(content, cap),
                ContentSource::WebFetch { url: url.clone() },
            ),
            extractor: extractor.clone(),
        });
        cache_store(key, bare_result.clone());
        apply_focus_prompt(bare_result, prompt.as_deref())
    }

    /// The per-result token budget Layer 2 holds this tool's result to: the
    /// resolution the dispatcher runs (declaration, default, window ceiling).
    fn result_budget_tokens() -> usize {
        crate::tools::result_processing::resolve_result_budget(
            Self::NAME,
            <Self as AlephTool>::MAX_RESULT_TOKENS,
        )
        .unwrap_or(crate::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS)
    }

    /// Characters of page text a result may carry: what `budget` allows, or
    /// less when the operator's `max_content_length` says less. The policy
    /// can only lower it — a larger one would be offloaded by Layer 2 anyway.
    fn content_cap(&self, budget: usize) -> usize {
        crate::context::budget::pressure::chars_for_result_token_budget(budget)
            .min(self.max_content_length)
    }

    /// This session's result store — the store `ctx_search` reads, scoped the
    /// way the dispatcher scopes it — and this call's id. `None` without a
    /// store.
    fn intent_store() -> Option<(
        std::sync::Arc<crate::tools::result_store::ToolResultStore>,
        String,
    )> {
        use crate::tools::result_store::{global_tool_result_store, ToolResultStore};
        let store = global_tool_result_store().map(|store| {
            match crate::tools::turn_context::current_session_key() {
                Some(session) => ToolResultStore::for_session(&store, session),
                None => store,
            }
        })?;
        let call_id = crate::approval::current_tool_call_id()
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        Some((store, call_id))
    }

    /// Extract the page title from <title> tag
    fn extract_title(&self, document: &Html) -> Option<String> {
        extract::extract_title(document)
    }

    /// Reject HTML that exceeds the 10 MB response budget to prevent `DoS`.
    pub(crate) fn validate_html_safety(html: &str) -> std::result::Result<(), ToolError> {
        extract::validate_html_safety(html, Self::MAX_RESPONSE_BYTES)
    }

    /// Cap fetched content at `cap` chars of SANITIZED text.
    ///
    /// The cap applies to what the model actually reads: sanitization inside
    /// [`wrap_external_content`] can grow the string (tokenizer markers
    /// become 23-char placeholders), so truncating raw text to the cap first
    /// would still let the fenced payload exceed it — and a raw cut can land
    /// inside a forged boundary marker, leaving a stub the sanitizer cannot
    /// see. `truncate_sanitized_external_content` solves both; the "..."
    /// suffix convention from the old raw truncation is preserved so
    /// downstream consumers still see the truncation signal.
    fn truncate_fetched(content: &str, cap: usize) -> String {
        let t =
            crate::security::content_sanitizer::truncate_sanitized_external_content(content, cap);
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
        max_chars: usize,
    ) -> (String, Extractor) {
        extract::extract_content_enhanced(
            raw_html,
            url,
            mode,
            self.enable_readability,
            self.min_content_length,
            max_chars,
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
    let Some(p) = focus_of(prompt) else {
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

/// The prompt, when it says anything.
fn focus_of(prompt: Option<&str>) -> Option<&str> {
    prompt.map(str::trim).filter(|s| !s.is_empty())
}

/// Attempts at fitting a result into its budget. What overshoots is the
/// envelope (URL, title, fence, focus marker, JSON escaping), which does not
/// shrink with the cap, so the proportional step lands in one or two.
const FIT_ATTEMPTS: usize = 4;

/// Tokens Layer 2 charges for `result`: it measures the flattened JSON of the
/// tool's output, so that is what is measured here.
fn charged_tokens(result: &WebFetchResult) -> usize {
    // Plain strings and a unit enum: serialization cannot fail.
    let flat = serde_json::to_string(result).unwrap_or_default();
    crate::context::budget::pressure::estimate_tokens_smart(&flat)
}

/// `build(cap)` for the largest cap (at most `cap`) whose result, with the
/// focus marker `apply_focus_prompt` will add, Layer 2 keeps inline under
/// `budget`. The returned result carries no marker; the caller adds it.
fn fit_to_budget(
    budget: usize,
    cap: usize,
    focus: Option<&str>,
    build: impl Fn(usize) -> WebFetchResult,
) -> WebFetchResult {
    let mut cap = cap;
    let mut result = build(cap);
    for _ in 0..FIT_ATTEMPTS {
        let charged = charged_tokens(&apply_focus_prompt(result.clone(), focus));
        if charged <= budget || cap == 0 {
            break;
        }
        let scaled = cap.saturating_mul(budget) / charged;
        cap = scaled - scaled / 20;
        result = build(cap);
    }
    result
}

/// Implementation of `AlephTool` trait for `WebFetchTool`
#[async_trait]
impl AlephTool for WebFetchTool {
    const NAME: &'static str = "web_fetch";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = WebFetchArgs;
    type Output = WebFetchResult;

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

    /// B4 wiring: a `prompt` on a page larger than the content cap takes
    /// the fetch-by-intent path through the process store; without a prompt
    /// the same page is truncated as before.
    ///
    /// Mutation-checked: never taking the intent branch in `finalize_success`
    /// turns this red.
    #[test]
    fn a_prompted_fetch_of_a_large_page_returns_the_matching_sections() {
        crate::tools::result_store::install_test_tool_result_store();
        let tool = WebFetchTool::new();
        let mut page = "Filler paragraph about nothing in particular.\n".repeat(600);
        page.push_str("The flux capacitor requires 1.21 gigawatts to operate.\n");
        page.push_str(&"More filler after the point.\n".repeat(200));
        let url = "https://example.com/intent-wiring";
        let fetch = |prompt: Option<&str>| {
            tool.finalize_success(
                WebFetchArgs {
                    url: url.to_string(),
                    extract_mode: ExtractMode::Markdown,
                    prompt: prompt.map(str::to_string),
                },
                cache_key(url, &ExtractMode::Markdown),
                None,
                &page,
                Extractor::Readability,
            )
        };
        let focused = fetch(Some("flux capacitor gigawatts"));
        assert!(
            focused.content.contains("1.21 gigawatts"),
            "{}",
            focused.content
        );
        assert!(
            focused.content.contains("matching the focus"),
            "{}",
            focused.content
        );
        assert!(focused.content.contains("[Full output persisted: "));

        let plain = fetch(None);
        assert!(!plain.content.contains("matching the focus"));
        assert!(
            !plain.content.contains("1.21 gigawatts"),
            "the plain fetch is cut at the cap"
        );
    }

    fn args(url: &str, prompt: Option<&str>) -> WebFetchArgs {
        WebFetchArgs {
            url: url.to_string(),
            extract_mode: ExtractMode::Markdown,
            prompt: prompt.map(str::to_string),
        }
    }

    /// `result` through Layer 2 as the dispatcher runs it: the ingress clean,
    /// then the per-result budget, under the budget the dispatcher resolves
    /// for this tool.
    fn through_layer_two(
        result: &WebFetchResult,
        call_id: &str,
        store: &crate::tools::result_store::ToolResultStore,
    ) -> crate::tools::result_processing::ProcessedResult {
        let mut value = serde_json::to_value(result).expect("serializable");
        let budget = crate::tools::result_processing::resolve_result_budget(
            WebFetchTool::NAME,
            <WebFetchTool as AlephTool>::MAX_RESULT_TOKENS,
        );
        let outcome =
            crate::tool_output::ingress::clean_for_ingress(WebFetchTool::NAME, &mut value, budget);
        crate::tools::result_processing::apply_result_budget(
            call_id,
            WebFetchTool::NAME,
            &outcome.model_facing,
            Some(store),
            budget,
            outcome.reduced_from.as_deref(),
            crate::tools::result_processing::RecoveryTools::ALL,
        )
    }

    /// A plain fetch of a page larger than the cap comes back from Layer 2
    /// inline, not offloaded behind a marker: the result is sized to the
    /// budget Layer 2 holds it to — envelope, fence and JSON escaping
    /// included — not to a character count that only the page text meets.
    ///
    /// Mutation-checked: skipping the fit (`FIT_ATTEMPTS = 0`) turns this red.
    #[test]
    fn a_plain_fetch_at_the_cap_stays_inline_through_layer_two() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let tool = WebFetchTool::new();
        let page = "Filler paragraph about \"nothing\" in particular.\n".repeat(600);
        let url = "https://example.com/plain-at-cap";
        let result = tool.finalize_success(
            args(url, None),
            cache_key(url, &ExtractMode::Markdown),
            Some("A page title".to_string()),
            &page,
            Extractor::Readability,
        );
        let cap = tool.content_cap(WebFetchTool::result_budget_tokens());
        assert!(
            result.content.chars().count() > cap / 2,
            "the page fills the result, not a sliver of it: {}",
            result.content.chars().count()
        );
        let processed = through_layer_two(&result, "call_plain_at_cap", &store);
        assert!(
            processed.persisted_path.is_none(),
            "offloaded: {}",
            processed.text
        );
        assert!(!processed.text.contains("[Full output persisted: "));
        assert!(processed.text.contains("Filler paragraph"));
    }

    /// A prompted fetch of a large page keeps the page's blob whole through
    /// Layer 2: the sections result, under the same call id, is never written
    /// over it.
    ///
    /// The prompt matches most of the page, so the selection fills its cap:
    /// a result that fits by luck would not reach the collision at all.
    ///
    /// Mutation-checked: skipping both the fit and Layer 2's own-marker check
    /// turns this red. Each alone is covered by its own test (the fit above,
    /// the check in `result_processing`).
    #[tokio::test]
    async fn a_prompted_fetch_keeps_the_whole_page_blob_through_layer_two() {
        let store = crate::tools::result_store::install_test_tool_result_store();
        let tool = WebFetchTool::new();
        let mut page = "Filler paragraph about nothing in particular.\n".repeat(600);
        page.push_str("The flux capacitor requires 1.21 gigawatts to operate.\n");
        page.push_str(&"More filler after the point.\n".repeat(200));
        page.push_str("The very last line of the page.\n");
        let url = "https://example.com/prompted-blob";
        let call_id = "call_prompted_blob";
        let identity = crate::approval::CallIdentity {
            turn_id: crate::session::events::TurnId::nil(),
            call_id: call_id.to_string(),
        };
        let result = crate::approval::with_call_identity(Some(identity), async {
            tool.finalize_success(
                args(url, Some("filler paragraph flux capacitor")),
                cache_key(url, &ExtractMode::Markdown),
                None,
                &page,
                Extractor::Readability,
            )
        })
        .await;
        assert!(
            result.content.contains("matching the focus"),
            "{}",
            result.content
        );

        assert!(
            result.content.chars().count()
                > tool.content_cap(WebFetchTool::result_budget_tokens()) / 2,
            "precondition: the selection fills the result: {}",
            result.content.chars().count()
        );

        let processed = through_layer_two(&result, call_id, &store);
        assert!(processed.persisted_path.is_none(), "persisted again");
        let blob = std::fs::read_to_string(store.blob_path(call_id, WebFetchTool::NAME))
            .expect("the page's blob");
        assert!(blob.contains("The very last line of the page."));
        assert!(
            !blob.contains("matching the focus"),
            "the blob is the page, not the sections result"
        );
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
        let cap = WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH;

        // Short content should not be truncated
        let short = "Hello world".to_string();
        assert_eq!(WebFetchTool::truncate_fetched(&short, cap), short);

        // Long content should be truncated
        let long = "a".repeat(15000);
        let truncated = WebFetchTool::truncate_fetched(&long, cap);
        assert!(truncated.chars().count() <= WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH + 3); // +3 for "..."
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn fetched_truncation_caps_the_sanitized_image() {
        // `<s>` is 3 raw chars but sanitizes to a 23-char placeholder. The
        // cap must absorb that growth, not be defeated by it.
        let hostile = "<s>".repeat(4000); // 12_000 raw chars → far over cap sanitized
        let out =
            WebFetchTool::truncate_fetched(&hostile, WebFetchTool::DEFAULT_MAX_CONTENT_LENGTH);
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

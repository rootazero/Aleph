//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements AlephTool trait for AI agent integration.

use async_trait::async_trait;
use super::error::ToolError;
use crate::config::WebFetchPolicy;
use crate::error::Result;
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::tools::AlephTool;
use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
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

/// Arguments for web fetch tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
    /// Content extraction mode (default: markdown)
    #[serde(default)]
    pub extract_mode: ExtractMode,
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

    /// Create a new WebFetchTool with default settings
    pub fn new() -> Self {
        Self {
            max_content_length: Self::DEFAULT_MAX_CONTENT_LENGTH,
            min_content_length: Self::DEFAULT_MIN_CONTENT_LENGTH,
            user_agent: Self::DEFAULT_USER_AGENT.to_string(),
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
        }
    }

    /// Create a new WebFetchTool with policy configuration
    pub fn with_policy(policy: &WebFetchPolicy) -> Self {
        Self {
            max_content_length: policy.max_content_length as usize,
            min_content_length: policy.min_content_length as usize,
            user_agent: policy.user_agent.clone(),
            timeout_secs: policy.timeout_seconds,
        }
    }

    /// Fetch and extract content from a URL (internal implementation)
    async fn call_impl(&self, args: WebFetchArgs) -> std::result::Result<WebFetchResult, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Notify tool start
        let url_display = if args.url.chars().count() > 50 {
            let truncated: String = args.url.chars().take(50).collect();
            format!("{}...", truncated)
        } else {
            args.url.clone()
        };
        notify_tool_start(Self::NAME, &format!("获取网页: {}", url_display));

        info!("Fetching URL: {}", args.url);

        // SSRF-protected fetch with DNS pinning
        let ssrf_policy = SsrfPolicy::default();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(ua) = reqwest::header::HeaderValue::from_str(&self.user_agent) {
            headers.insert(reqwest::header::USER_AGENT, ua);
        }
        let fetch_request = SafeFetchRequest::get(std::time::Duration::from_secs(self.timeout_secs))
            .with_headers(headers);

        let fetch_response = safe_fetch(&args.url, &ssrf_policy, fetch_request)
            .await
            .map_err(|e| {
                let error_msg = format!("Fetch blocked or failed: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Network(error_msg)
            })?;

        if !fetch_response.status.is_success() {
            let error_msg = format!("HTTP error: {} for URL: {}", fetch_response.status, args.url);
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

        let html_content = String::from_utf8_lossy(&bytes).to_string();

        debug!("Fetched {} bytes from {}", html_content.len(), args.url);

        // Parse HTML
        let document = Html::parse_document(&html_content);

        // Extract title
        let title = self.extract_title(&document);
        debug!("Extracted title: {:?}", title);

        // Extract main content
        let content = self.extract_content(&document);
        debug!("Extracted {} chars of content", content.len());

        // Notify success
        let result_summary = format!(
            "已获取网页内容 ({} 字符)",
            content.len()
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        // Wrap fetched content with external content boundary markers to guard
        // against prompt injection embedded in web page content.
        let wrapped_content = wrap_external_content(
            &content,
            ContentSource::WebFetch { url: args.url.clone() },
        );

        Ok(WebFetchResult {
            url: args.url,
            title,
            content: wrapped_content,
            extractor: Extractor::Selector,
        })
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
            format!("{}...", truncated)
        }
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
        }
    }
}

/// Implementation of AlephTool trait for WebFetchTool
#[async_trait]
impl AlephTool for WebFetchTool {
    const NAME: &'static str = "web_fetch";
    const DESCRIPTION: &'static str = "Fetch and extract text content from a web page URL.";

    type Args = WebFetchArgs;
    type Output = WebFetchResult;

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
        };

        // Use fully qualified syntax to avoid ambiguity
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err(), "Expected error for invalid URL");

        // Error is now AlephError wrapping the SSRF/fetch error
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Fetch blocked or failed"), "Expected 'Fetch blocked or failed' error, got: {}", err_msg);
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
        let args: WebFetchArgs = serde_json::from_str(
            r#"{"url": "https://example.com", "extract_mode": "text"}"#
        ).unwrap();
        assert!(matches!(args.extract_mode, ExtractMode::Text));
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
}

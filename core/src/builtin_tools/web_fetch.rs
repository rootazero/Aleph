//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements AlephTool trait for AI agent integration.

use async_trait::async_trait;
use super::error::ToolError;
use crate::config::WebFetchPolicy;
use crate::error::Result;
use crate::tools::AlephTool;
use reqwest::Client;
use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use tracing::{debug, info};
use url::Url;


/// Arguments for web fetch tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchArgs {
    /// URL to fetch
    pub url: String,
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
}

/// Web fetch tool for retrieving and extracting content from web pages
pub struct WebFetchTool {
    client: Client,
    /// Maximum content length in characters (from policy)
    max_content_length: usize,
    /// Minimum content length to accept a selector match (from policy)
    min_content_length: usize,
    /// User agent string (from policy)
    user_agent: String,
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

    /// Create a new WebFetchTool with default settings
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            max_content_length: Self::DEFAULT_MAX_CONTENT_LENGTH,
            min_content_length: Self::DEFAULT_MIN_CONTENT_LENGTH,
            user_agent: Self::DEFAULT_USER_AGENT.to_string(),
        }
    }

    /// Create a new WebFetchTool with policy configuration
    pub fn with_policy(policy: &WebFetchPolicy) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(policy.timeout_seconds))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            max_content_length: policy.max_content_length as usize,
            min_content_length: policy.min_content_length as usize,
            user_agent: policy.user_agent.clone(),
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

        // Validate URL format
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            let error_msg = format!(
                "Invalid URL format: {}. URL must start with http:// or https://",
                args.url
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::InvalidArgs(error_msg));
        }

        // SSRF protection: block requests to internal/private hosts
        if let Err(reason) = is_safe_url(&args.url) {
            let error_msg = format!("URL blocked (SSRF protection): {}", reason);
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::InvalidArgs(error_msg));
        }

        // Fetch the page
        let response = self
            .client
            .get(&args.url)
            .header("User-Agent", &self.user_agent)
            .send()
            .await
            .map_err(|e| {
                let error_msg = format!("Failed to fetch URL: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Network(error_msg)
            })?;

        // Check status
        if !response.status().is_success() {
            let error_msg = format!("HTTP error: {} for URL: {}", response.status(), args.url);
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Network(error_msg));
        }

        // Get HTML content
        let html_content = response.text().await.map_err(|e| {
            let error_msg = format!("Failed to read response body: {}", e);
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::Network(error_msg)
        })?;

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

        Ok(WebFetchResult {
            url: args.url,
            title,
            content,
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
        if content.len() <= self.max_content_length {
            content
        } else {
            // Truncate at character boundary
            let truncated: String = content.chars().take(self.max_content_length).collect();
            format!("{}...", truncated)
        }
    }
}

/// Check if a URL is safe to fetch (not targeting internal/private services)
fn is_safe_url(url_str: &str) -> std::result::Result<(), String> {
    let parsed = Url::parse(url_str).map_err(|e| format!("invalid URL: {}", e))?;

    let host = parsed.host_str().ok_or("URL has no host")?;

    // Block known dangerous hostnames
    let blocked_hostnames = [
        "localhost",
        "metadata.google.internal",
        "metadata.google",
    ];
    let lower_host = host.to_lowercase();
    for blocked in &blocked_hostnames {
        if lower_host == *blocked {
            return Err(format!("blocked host: {}", host));
        }
    }

    // Try to parse as IP directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        return check_ip_safety(ip);
    }

    // Resolve hostname to IP addresses and check each one
    let port = parsed.port().unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let socket_addr = format!("{}:{}", host, port);
    if let Ok(addrs) = socket_addr.to_socket_addrs() {
        for addr in addrs {
            check_ip_safety(addr.ip())?;
        }
    }

    Ok(())
}

/// Check if an IP address is safe (not loopback, private, link-local, or cloud metadata)
fn check_ip_safety(ip: IpAddr) -> std::result::Result<(), String> {
    match ip {
        IpAddr::V4(ipv4) => {
            if ipv4.is_loopback() {
                return Err(format!("loopback address: {}", ipv4));
            }
            if ipv4.is_private() {
                return Err(format!("private network address: {}", ipv4));
            }
            if ipv4.is_link_local() {
                return Err(format!("link-local address: {}", ipv4));
            }
            // AWS/cloud metadata endpoint: 169.254.169.254
            if ipv4 == Ipv4Addr::new(169, 254, 169, 254) {
                return Err("cloud metadata endpoint".to_string());
            }
            // Block 0.0.0.0/8
            if ipv4.octets()[0] == 0 {
                return Err(format!("unspecified address: {}", ipv4));
            }
        }
        IpAddr::V6(ipv6) => {
            if ipv6.is_loopback() {
                return Err(format!("loopback address: {}", ipv6));
            }
            // IPv4-mapped IPv6 addresses (::ffff:x.x.x.x)
            if let Some(mapped) = ipv6.to_ipv4_mapped() {
                return check_ip_safety(IpAddr::V4(mapped));
            }
            // IPv6 unspecified
            if ipv6 == Ipv6Addr::UNSPECIFIED {
                return Err("unspecified address: ::".to_string());
            }
        }
    }
    Ok(())
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WebFetchTool {
    fn clone(&self) -> Self {
        // Rebuild client with same settings is tricky, use default timeout for now
        // The policy values are preserved
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            max_content_length: self.max_content_length,
            min_content_length: self.min_content_length,
            user_agent: self.user_agent.clone(),
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
    use crate::tools::AlephTool;

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
        // Verify the tool was created (client is private, so we just ensure no panic)
        drop(tool);
    }

    #[tokio::test]
    #[ignore] // Requires network connection
    async fn test_web_fetch_call() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs {
            url: "https://example.com".to_string(),
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
        };

        // Use fully qualified syntax to avoid ambiguity
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err(), "Expected error for invalid URL");

        // Error is now AlephError
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("Invalid URL format"), "Expected 'Invalid URL format' error, got: {}", err_msg);
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
        assert!(is_safe_url("http://localhost/admin").is_err());
        assert!(is_safe_url("http://127.0.0.1/secret").is_err());
        assert!(is_safe_url("http://127.0.0.1:8080/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_networks() {
        assert!(is_safe_url("http://10.0.0.1/internal").is_err());
        assert!(is_safe_url("http://192.168.1.1/admin").is_err());
        assert!(is_safe_url("http://172.16.0.1/secret").is_err());
    }

    #[test]
    fn test_ssrf_blocks_metadata_endpoints() {
        assert!(is_safe_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(is_safe_url("http://metadata.google.internal/computeMetadata/").is_err());
    }

    #[test]
    fn test_ssrf_allows_public_urls() {
        assert!(is_safe_url("https://example.com").is_ok());
        assert!(is_safe_url("https://8.8.8.8").is_ok());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_loopback() {
        assert!(is_safe_url("http://[::1]/admin").is_err());
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
}

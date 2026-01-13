//! Web Fetch Tool
//!
//! Fetches web pages and extracts readable content in Markdown format.

use async_trait::async_trait;
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::error::{AetherError, Result};
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for WebFetchTool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchConfig {
    /// Whether web fetch is enabled
    pub enabled: bool,

    /// Maximum content length to return (bytes)
    pub max_content_bytes: usize,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    /// User agent string
    pub user_agent: String,

    /// Blocked domains (e.g., localhost, internal networks)
    pub blocked_domains: Vec<String>,

    /// Maximum redirects to follow
    pub max_redirects: usize,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_content_bytes: 100 * 1024, // 100KB
            timeout_seconds: 30,
            user_agent: "Mozilla/5.0 (compatible; AetherBot/1.0)".to_string(),
            blocked_domains: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "0.0.0.0".to_string(),
                "::1".to_string(),
            ],
            max_redirects: 5,
        }
    }
}

// =============================================================================
// Parameters
// =============================================================================

/// Parameters for web_fetch tool
#[derive(Debug, Deserialize)]
struct WebFetchParams {
    /// URL to fetch
    url: String,

    /// Whether to include links in the output (default: false)
    #[serde(default)]
    include_links: bool,

    /// Whether to include images as markdown references (default: false)
    #[serde(default)]
    include_images: bool,
}

// =============================================================================
// WebFetchTool
// =============================================================================

/// Tool for fetching and extracting web page content
///
/// This tool:
/// 1. Fetches HTML from a URL
/// 2. Extracts the main content (article, main, body)
/// 3. Converts HTML to Markdown format
/// 4. Truncates content if too long
pub struct WebFetchTool {
    client: Client,
    config: WebFetchConfig,
}

impl WebFetchTool {
    /// Create a new WebFetchTool with configuration
    pub fn new(config: WebFetchConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .redirect(reqwest::redirect::Policy::limited(config.max_redirects))
            .user_agent(&config.user_agent)
            .build()
            .unwrap_or_default();

        Self { client, config }
    }

    /// Validate URL before fetching
    fn validate_url(&self, url: &url::Url) -> Result<()> {
        // Check scheme
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(AetherError::other(format!(
                "Invalid URL scheme '{}'. Only http and https are supported.",
                url.scheme()
            )));
        }

        // Check for blocked domains
        if let Some(host) = url.host_str() {
            let host_lower = host.to_lowercase();
            for blocked in &self.config.blocked_domains {
                if host_lower == blocked.to_lowercase() || host_lower.ends_with(&format!(".{}", blocked.to_lowercase())) {
                    return Err(AetherError::other(format!(
                        "Domain '{}' is blocked for security reasons.",
                        host
                    )));
                }
            }

            // Block private IP ranges
            if host_lower.starts_with("10.")
                || host_lower.starts_with("192.168.")
                || host_lower.starts_with("172.16.")
                || host_lower.starts_with("172.17.")
                || host_lower.starts_with("172.18.")
                || host_lower.starts_with("172.19.")
                || host_lower.starts_with("172.20.")
                || host_lower.starts_with("172.21.")
                || host_lower.starts_with("172.22.")
                || host_lower.starts_with("172.23.")
                || host_lower.starts_with("172.24.")
                || host_lower.starts_with("172.25.")
                || host_lower.starts_with("172.26.")
                || host_lower.starts_with("172.27.")
                || host_lower.starts_with("172.28.")
                || host_lower.starts_with("172.29.")
                || host_lower.starts_with("172.30.")
                || host_lower.starts_with("172.31.")
            {
                return Err(AetherError::other(
                    "Private IP addresses are blocked for security reasons.",
                ));
            }
        }

        Ok(())
    }

    /// Extract readable content from HTML
    fn extract_content(&self, html: &str, include_links: bool, include_images: bool) -> String {
        let document = Html::parse_document(html);

        // Extract title
        let title = self.extract_title(&document);

        // Try to find main content area
        let content_selectors = [
            "article",
            "main",
            "[role='main']",
            ".post-content",
            ".article-content",
            ".entry-content",
            ".content",
            "#content",
            ".post",
            ".article",
        ];

        let mut content = String::new();

        for selector_str in content_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = document.select(&selector).next() {
                    content = self.element_to_markdown(&element, include_links, include_images, 0);
                    if !content.trim().is_empty() && content.trim().len() > 100 {
                        break;
                    }
                }
            }
        }

        // Fallback to body if no main content found
        if content.trim().is_empty() || content.trim().len() < 100 {
            if let Ok(selector) = Selector::parse("body") {
                if let Some(element) = document.select(&selector).next() {
                    content = self.element_to_markdown(&element, include_links, include_images, 0);
                }
            }
        }

        // Clean up whitespace
        let content = self.clean_whitespace(&content);

        // Format with title
        if let Some(title) = title {
            format!("# {}\n\n{}", title.trim(), content.trim())
        } else {
            content.trim().to_string()
        }
    }

    /// Extract page title
    fn extract_title(&self, document: &Html) -> Option<String> {
        // Try og:title first
        if let Ok(selector) = Selector::parse("meta[property='og:title']") {
            if let Some(element) = document.select(&selector).next() {
                if let Some(content) = element.value().attr("content") {
                    if !content.is_empty() {
                        return Some(content.to_string());
                    }
                }
            }
        }

        // Fall back to title tag
        if let Ok(selector) = Selector::parse("title") {
            if let Some(element) = document.select(&selector).next() {
                let title = element.text().collect::<String>();
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }

        None
    }

    /// Convert HTML element to Markdown
    fn element_to_markdown(
        &self,
        element: &ElementRef,
        include_links: bool,
        include_images: bool,
        depth: usize,
    ) -> String {
        // Prevent infinite recursion
        if depth > 20 {
            return String::new();
        }

        // Tags to skip entirely
        let skip_tags: HashSet<&str> = [
            "script", "style", "nav", "footer", "aside", "header", "noscript",
            "iframe", "form", "button", "input", "select", "textarea", "svg",
            "canvas", "video", "audio", "source", "track", "embed", "object",
            "param", "map", "area", "template", "slot", "menu", "dialog",
        ]
        .iter()
        .copied()
        .collect();

        let tag = element.value().name();

        // Skip certain tags
        if skip_tags.contains(tag) {
            return String::new();
        }

        // Skip elements with common ad/navigation classes
        if let Some(class) = element.value().attr("class") {
            let class_lower = class.to_lowercase();
            if class_lower.contains("nav")
                || class_lower.contains("menu")
                || class_lower.contains("sidebar")
                || class_lower.contains("footer")
                || class_lower.contains("header")
                || class_lower.contains("ad-")
                || class_lower.contains("advertisement")
                || class_lower.contains("social")
                || class_lower.contains("share")
                || class_lower.contains("comment")
                || class_lower.contains("related")
            {
                return String::new();
            }
        }

        let mut result = String::new();

        match tag {
            // Headings
            "h1" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("\n\n## {}\n\n", text.trim()));
                }
            }
            "h2" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("\n\n### {}\n\n", text.trim()));
                }
            }
            "h3" | "h4" | "h5" | "h6" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("\n\n#### {}\n\n", text.trim()));
                }
            }

            // Paragraphs
            "p" => {
                let text = self.process_inline_children(element, include_links, include_images, depth);
                if !text.trim().is_empty() {
                    result.push_str(&format!("\n\n{}\n\n", text.trim()));
                }
            }

            // Lists
            "ul" | "ol" => {
                result.push_str("\n\n");
                let mut list_index = 0;
                for child in element.children() {
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        if child_elem.value().name() == "li" {
                            list_index += 1;
                            let text = self.process_inline_children(&child_elem, include_links, include_images, depth + 1);
                            if !text.trim().is_empty() {
                                if tag == "ol" {
                                    result.push_str(&format!("{}. {}\n", list_index, text.trim()));
                                } else {
                                    result.push_str(&format!("- {}\n", text.trim()));
                                }
                            }
                        }
                    }
                }
                result.push('\n');
            }

            // Blockquotes
            "blockquote" => {
                let text = self.process_inline_children(element, include_links, include_images, depth + 1);
                if !text.trim().is_empty() {
                    let quoted = text.lines()
                        .map(|line| format!("> {}", line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push_str(&format!("\n\n{}\n\n", quoted));
                }
            }

            // Code blocks
            "pre" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("\n\n```\n{}\n```\n\n", text.trim()));
                }
            }
            "code" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("`{}`", text));
                }
            }

            // Links
            "a" if include_links => {
                let text = self.get_text_content(element);
                if let Some(href) = element.value().attr("href") {
                    if !text.is_empty() && !href.is_empty() && !href.starts_with('#') && !href.starts_with("javascript:") {
                        result.push_str(&format!("[{}]({})", text.trim(), href));
                    } else if !text.is_empty() {
                        result.push_str(&text);
                    }
                } else if !text.is_empty() {
                    result.push_str(&text);
                }
            }
            "a" => {
                result.push_str(&self.get_text_content(element));
            }

            // Images
            "img" if include_images => {
                if let Some(alt) = element.value().attr("alt") {
                    if let Some(src) = element.value().attr("src") {
                        if !src.is_empty() {
                            result.push_str(&format!("![{}]({})", alt, src));
                        }
                    }
                }
            }

            // Inline formatting
            "strong" | "b" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("**{}**", text));
                }
            }
            "em" | "i" => {
                let text = self.get_text_content(element);
                if !text.is_empty() {
                    result.push_str(&format!("*{}*", text));
                }
            }

            // Line breaks
            "br" => {
                result.push_str("\n");
            }
            "hr" => {
                result.push_str("\n\n---\n\n");
            }

            // Divs and spans - process children
            "div" | "span" | "section" | "article" | "main" | "figure" | "figcaption" => {
                for child in element.children() {
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        result.push_str(&self.element_to_markdown(&child_elem, include_links, include_images, depth + 1));
                    } else if let Some(text) = child.value().as_text() {
                        let text = text.trim();
                        if !text.is_empty() {
                            result.push_str(text);
                            result.push(' ');
                        }
                    }
                }
            }

            // Table handling (simplified)
            "table" => {
                result.push_str("\n\n");
                for child in element.children() {
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        let child_tag = child_elem.value().name();
                        if child_tag == "thead" || child_tag == "tbody" || child_tag == "tr" {
                            result.push_str(&self.element_to_markdown(&child_elem, include_links, include_images, depth + 1));
                        }
                    }
                }
                result.push_str("\n");
            }
            "tr" => {
                result.push_str("| ");
                for child in element.children() {
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        let child_tag = child_elem.value().name();
                        if child_tag == "td" || child_tag == "th" {
                            let text = self.get_text_content(&child_elem);
                            result.push_str(&format!("{} | ", text.trim()));
                        }
                    }
                }
                result.push('\n');
            }

            // Default: process children
            _ => {
                for child in element.children() {
                    if let Some(child_elem) = ElementRef::wrap(child) {
                        result.push_str(&self.element_to_markdown(&child_elem, include_links, include_images, depth + 1));
                    } else if let Some(text) = child.value().as_text() {
                        let text = text.trim();
                        if !text.is_empty() {
                            result.push_str(text);
                            result.push(' ');
                        }
                    }
                }
            }
        }

        result
    }

    /// Process inline children (for paragraphs, list items, etc.)
    fn process_inline_children(
        &self,
        element: &ElementRef,
        include_links: bool,
        include_images: bool,
        depth: usize,
    ) -> String {
        let mut result = String::new();

        for child in element.children() {
            if let Some(child_elem) = ElementRef::wrap(child) {
                let tag = child_elem.value().name();
                match tag {
                    "strong" | "b" => {
                        let text = self.get_text_content(&child_elem);
                        if !text.is_empty() {
                            result.push_str(&format!("**{}**", text));
                        }
                    }
                    "em" | "i" => {
                        let text = self.get_text_content(&child_elem);
                        if !text.is_empty() {
                            result.push_str(&format!("*{}*", text));
                        }
                    }
                    "code" => {
                        let text = self.get_text_content(&child_elem);
                        if !text.is_empty() {
                            result.push_str(&format!("`{}`", text));
                        }
                    }
                    "a" if include_links => {
                        let text = self.get_text_content(&child_elem);
                        if let Some(href) = child_elem.value().attr("href") {
                            if !text.is_empty() && !href.is_empty() && !href.starts_with('#') && !href.starts_with("javascript:") {
                                result.push_str(&format!("[{}]({})", text.trim(), href));
                            } else if !text.is_empty() {
                                result.push_str(&text);
                            }
                        } else if !text.is_empty() {
                            result.push_str(&text);
                        }
                    }
                    "a" => {
                        result.push_str(&self.get_text_content(&child_elem));
                    }
                    "br" => {
                        result.push('\n');
                    }
                    "img" if include_images => {
                        if let Some(alt) = child_elem.value().attr("alt") {
                            if let Some(src) = child_elem.value().attr("src") {
                                if !src.is_empty() {
                                    result.push_str(&format!("![{}]({})", alt, src));
                                }
                            }
                        }
                    }
                    _ => {
                        result.push_str(&self.element_to_markdown(&child_elem, include_links, include_images, depth + 1));
                    }
                }
            } else if let Some(text) = child.value().as_text() {
                result.push_str(text);
            }
        }

        result
    }

    /// Get text content of an element (recursive)
    fn get_text_content(&self, element: &ElementRef) -> String {
        let mut result = String::new();

        for child in element.children() {
            if let Some(child_elem) = ElementRef::wrap(child) {
                result.push_str(&self.get_text_content(&child_elem));
            } else if let Some(text) = child.value().as_text() {
                result.push_str(text);
            }
        }

        result
    }

    /// Clean up excessive whitespace
    fn clean_whitespace(&self, text: &str) -> String {
        let mut result = String::new();
        let mut prev_was_newline = false;
        let mut newline_count = 0;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                newline_count += 1;
                if newline_count <= 2 {
                    result.push('\n');
                }
                prev_was_newline = true;
            } else {
                if prev_was_newline && newline_count > 0 {
                    // Ensure at most 2 newlines between content
                }
                result.push_str(trimmed);
                result.push('\n');
                prev_was_newline = false;
                newline_count = 0;
            }
        }

        result
    }
}

#[async_trait]
impl AgentTool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "web_fetch",
            "Fetch and extract readable content from a web page URL. Returns the main text content in Markdown format. Use this when users ask to read, summarize, or access content from a URL.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL of the web page to fetch (must be http or https)"
                    },
                    "include_links": {
                        "type": "boolean",
                        "description": "Whether to preserve hyperlinks as Markdown links (default: false)"
                    },
                    "include_images": {
                        "type": "boolean",
                        "description": "Whether to include image references (default: false)"
                    }
                },
                "required": ["url"]
            }),
            ToolCategory::Native, // Using Search category since it's web-related
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: WebFetchParams = serde_json::from_str(args).map_err(|e| {
            AetherError::other(format!("Invalid parameters: {}. Expected JSON with 'url' field.", e))
        })?;

        info!(url = %params.url, "WebFetchTool: Fetching URL");

        // Parse and validate URL
        let url = url::Url::parse(&params.url).map_err(|e| {
            AetherError::other(format!("Invalid URL '{}': {}", params.url, e))
        })?;

        self.validate_url(&url)?;

        // Fetch the page
        let response = self
            .client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AetherError::other(format!("Request timed out after {} seconds", self.config.timeout_seconds))
                } else if e.is_connect() {
                    AetherError::other(format!("Failed to connect to {}: {}", url.host_str().unwrap_or("unknown"), e))
                } else {
                    AetherError::other(format!("HTTP request failed: {}", e))
                }
            })?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            return Ok(ToolResult::error(format!(
                "HTTP {} {} for URL: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                url
            )));
        }

        // Check content type
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html");

        if !content_type.contains("text/html") && !content_type.contains("text/plain") && !content_type.contains("application/xhtml") {
            warn!(content_type = %content_type, "Non-HTML content type");
            // Still try to process it, but warn
        }

        // Get body text
        let html = response.text().await.map_err(|e| {
            AetherError::other(format!("Failed to read response body: {}", e))
        })?;

        debug!(html_length = html.len(), "Received HTML content");

        // Extract content
        let content = self.extract_content(&html, params.include_links, params.include_images);

        // Truncate if needed
        let content = if content.len() > self.config.max_content_bytes {
            let truncated: String = content
                .chars()
                .take(self.config.max_content_bytes)
                .collect();
            format!(
                "{}\n\n[Content truncated. Original size: {} bytes, limit: {} bytes]",
                truncated,
                content.len(),
                self.config.max_content_bytes
            )
        } else {
            content
        };

        info!(
            url = %params.url,
            content_length = content.len(),
            "WebFetchTool: Successfully extracted content"
        );

        Ok(ToolResult::success(content))
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Native
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> WebFetchTool {
        WebFetchTool::new(WebFetchConfig::default())
    }

    #[test]
    fn test_config_default() {
        let config = WebFetchConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_content_bytes, 100 * 1024);
        assert_eq!(config.timeout_seconds, 30);
        assert!(!config.blocked_domains.is_empty());
    }

    #[test]
    fn test_tool_definition() {
        let tool = create_test_tool();
        let def = tool.definition();

        assert_eq!(def.name, "web_fetch");
        assert!(def.description.contains("Fetch"));
        assert!(!def.requires_confirmation);
        assert_eq!(def.category, ToolCategory::Native);
    }

    #[test]
    fn test_validate_url_valid() {
        let tool = create_test_tool();

        let url = url::Url::parse("https://example.com").unwrap();
        assert!(tool.validate_url(&url).is_ok());

        let url = url::Url::parse("http://example.com/path?query=1").unwrap();
        assert!(tool.validate_url(&url).is_ok());
    }

    #[test]
    fn test_validate_url_invalid_scheme() {
        let tool = create_test_tool();

        let url = url::Url::parse("ftp://example.com").unwrap();
        assert!(tool.validate_url(&url).is_err());

        let url = url::Url::parse("file:///etc/passwd").unwrap();
        assert!(tool.validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_blocked_domain() {
        let tool = create_test_tool();

        let url = url::Url::parse("http://localhost/admin").unwrap();
        assert!(tool.validate_url(&url).is_err());

        let url = url::Url::parse("http://127.0.0.1:8080").unwrap();
        assert!(tool.validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_private_ip() {
        let tool = create_test_tool();

        let url = url::Url::parse("http://192.168.1.1").unwrap();
        assert!(tool.validate_url(&url).is_err());

        let url = url::Url::parse("http://10.0.0.1").unwrap();
        assert!(tool.validate_url(&url).is_err());
    }

    #[test]
    fn test_extract_simple_html() {
        let tool = create_test_tool();

        let html = r#"
            <html>
            <head><title>Test Page</title></head>
            <body>
                <article>
                    <h1>Hello World</h1>
                    <p>This is a test paragraph.</p>
                </article>
            </body>
            </html>
        "#;

        let content = tool.extract_content(html, false, false);

        assert!(content.contains("# Test Page"));
        assert!(content.contains("Hello World"));
        assert!(content.contains("This is a test paragraph"));
    }

    #[test]
    fn test_extract_with_links() {
        let tool = create_test_tool();

        let html = r#"
            <html>
            <body>
                <p>Check out <a href="https://example.com">this link</a>!</p>
            </body>
            </html>
        "#;

        // With links
        let content = tool.extract_content(html, true, false);
        assert!(content.contains("[this link](https://example.com)"));

        // Without links
        let content = tool.extract_content(html, false, false);
        assert!(!content.contains("[this link]"));
        assert!(content.contains("this link"));
    }

    #[test]
    fn test_extract_strips_scripts() {
        let tool = create_test_tool();

        let html = r#"
            <html>
            <body>
                <p>Visible text</p>
                <script>alert('evil');</script>
                <style>.hidden { display: none; }</style>
                <p>More visible text</p>
            </body>
            </html>
        "#;

        let content = tool.extract_content(html, false, false);

        assert!(content.contains("Visible text"));
        assert!(content.contains("More visible text"));
        assert!(!content.contains("alert"));
        assert!(!content.contains("display: none"));
    }

    #[test]
    fn test_extract_list() {
        let tool = create_test_tool();

        let html = r#"
            <html>
            <body>
                <ul>
                    <li>Item 1</li>
                    <li>Item 2</li>
                </ul>
                <ol>
                    <li>First</li>
                    <li>Second</li>
                </ol>
            </body>
            </html>
        "#;

        let content = tool.extract_content(html, false, false);

        assert!(content.contains("- Item 1"));
        assert!(content.contains("- Item 2"));
        assert!(content.contains("1. First"));
        assert!(content.contains("2. Second"));
    }

    #[test]
    fn test_extract_code_block() {
        let tool = create_test_tool();

        let html = r#"
            <html>
            <body>
                <pre>fn main() {
    println!("Hello");
}</pre>
            </body>
            </html>
        "#;

        let content = tool.extract_content(html, false, false);

        assert!(content.contains("```"));
        assert!(content.contains("fn main()"));
    }

    #[tokio::test]
    async fn test_execute_invalid_url() {
        let tool = create_test_tool();

        let result = tool.execute(r#"{"url": "not-a-url"}"#).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_missing_url() {
        let tool = create_test_tool();

        let result = tool.execute(r#"{}"#).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_blocked_url() {
        let tool = create_test_tool();

        let result = tool.execute(r#"{"url": "http://localhost/admin"}"#).await;
        assert!(result.is_err());
    }
}

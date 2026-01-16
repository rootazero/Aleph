//! Web fetch tool for retrieving and extracting content from web pages
//!
//! Implements rig's Tool trait for AI agent integration.

use crate::config::WebFetchPolicy;
use reqwest::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use super::error::ToolError;

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
    pub const DESCRIPTION: &'static str =
        "Fetch and extract text content from a web page URL.";

    /// Default maximum content length (used when no policy provided)
    const DEFAULT_MAX_CONTENT_LENGTH: usize = 10000;

    /// Default minimum content length (used when no policy provided)
    const DEFAULT_MIN_CONTENT_LENGTH: usize = 100;

    /// Default user agent string (used when no policy provided)
    const DEFAULT_USER_AGENT: &'static str = "Aether/1.0";

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
            max_content_length: policy.max_content_length,
            min_content_length: policy.min_content_length,
            user_agent: policy.user_agent.clone(),
        }
    }

    /// Fetch and extract content from a URL
    pub async fn call(&self, args: WebFetchArgs) -> Result<WebFetchResult, ToolError> {
        info!("Fetching URL: {}", args.url);

        // Validate URL format
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Err(ToolError::InvalidArgs(format!(
                "Invalid URL format: {}. URL must start with http:// or https://",
                args.url
            )));
        }

        // Fetch the page
        let response = self
            .client
            .get(&args.url)
            .header("User-Agent", &self.user_agent)
            .send()
            .await
            .map_err(|e| ToolError::Network(format!("Failed to fetch URL: {}", e)))?;

        // Check status
        if !response.status().is_success() {
            return Err(ToolError::Network(format!(
                "HTTP error: {} for URL: {}",
                response.status(),
                args.url
            )));
        }

        // Get HTML content
        let html_content = response
            .text()
            .await
            .map_err(|e| ToolError::Network(format!("Failed to read response body: {}", e)))?;

        debug!("Fetched {} bytes from {}", html_content.len(), args.url);

        // Parse HTML
        let document = Html::parse_document(&html_content);

        // Extract title
        let title = self.extract_title(&document);
        debug!("Extracted title: {:?}", title);

        // Extract main content
        let content = self.extract_content(&document);
        debug!("Extracted {} chars of content", content.len());

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

/// Implementation of rig's Tool trait for WebFetchTool
///
/// This allows WebFetchTool to be used with rig agents via the `.tool()` method.
impl Tool for WebFetchTool {
    const NAME: &'static str = "web_fetch";

    type Error = ToolError;
    type Args = WebFetchArgs;
    type Output = WebFetchResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        // Use schemars to generate JSON Schema for the arguments
        let schema = schema_for!(WebFetchArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            // Fallback to manually defined schema if generation fails
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch (must start with http:// or https://)"
                    }
                },
                "required": ["url"]
            })
        });

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Delegate to the existing call implementation
        WebFetchTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_args() {
        let args: WebFetchArgs =
            serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
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
    async fn test_web_fetch_call() {
        let tool = WebFetchTool::new();
        let args = WebFetchArgs {
            url: "https://example.com".to_string(),
        };

        let result = tool.call(args).await;
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

        let result = tool.call(args).await;
        assert!(result.is_err(), "Expected error for invalid URL");

        let err = result.unwrap_err();
        match err {
            ToolError::InvalidArgs(msg) => {
                assert!(msg.contains("Invalid URL format"));
            }
            _ => panic!("Expected InvalidArgs error, got: {:?}", err),
        }
    }

    #[test]
    fn test_clean_text() {
        let tool = WebFetchTool::new();
        let text = "  Hello   world  \n\t  test  ";
        let cleaned = tool.clean_text(text);
        assert_eq!(cleaned, "Hello world test");
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

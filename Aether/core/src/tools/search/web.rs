//! Web Search Tool
//!
//! Provides web search via the AgentTool trait.

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{AetherError, Result};
use crate::search::{SearchOptions, SearchRegistry, SearchResult};
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Web search configuration
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Whether search is enabled
    pub enabled: bool,
    /// Default maximum results
    pub default_max_results: usize,
    /// Default timeout in seconds
    pub default_timeout_seconds: u64,
    /// Whether to apply PII scrubbing to queries
    pub pii_scrubbing: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_max_results: 5,
            default_timeout_seconds: 10,
            pii_scrubbing: true,
        }
    }
}

impl SearchConfig {
    /// Create a disabled configuration
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

/// Search tools context
///
/// Provides shared access to search registry and configuration.
#[derive(Clone)]
pub struct SearchContext {
    /// Search registry (set after initialization)
    registry: Arc<Mutex<Option<SearchRegistry>>>,
    /// Configuration
    config: Arc<SearchConfig>,
}

impl SearchContext {
    /// Create a new context
    pub fn new(config: SearchConfig) -> Self {
        Self {
            registry: Arc::new(Mutex::new(None)),
            config: Arc::new(config),
        }
    }

    /// Set the search registry (called after SearchRegistry is initialized)
    pub async fn set_registry(&self, registry: SearchRegistry) {
        let mut guard = self.registry.lock().await;
        *guard = Some(registry);
    }

    /// Check if search is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get registry handle for external setup
    pub fn registry_handle(&self) -> Arc<Mutex<Option<SearchRegistry>>> {
        Arc::clone(&self.registry)
    }
}

impl Default for SearchContext {
    fn default() -> Self {
        Self::new(SearchConfig::default())
    }
}

/// Parameters for web_search tool
#[derive(Debug, Deserialize)]
struct WebSearchParams {
    /// Search query
    query: String,
    /// Maximum results (optional)
    #[serde(default)]
    max_results: Option<usize>,
    /// Language code (optional)
    #[serde(default)]
    language: Option<String>,
    /// Date range filter (optional)
    #[serde(default)]
    date_range: Option<String>,
}

/// Web search tool
///
/// Searches the web for real-time information.
/// Uses the configured search registry (Tavily, Brave, Google, etc.)
pub struct WebSearchTool {
    ctx: SearchContext,
}

impl WebSearchTool {
    /// Create a new WebSearchTool with the given context
    pub fn new(ctx: SearchContext) -> Self {
        Self { ctx }
    }

    /// Convert search results to formatted output
    fn format_results(results: &[SearchResult]) -> String {
        if results.is_empty() {
            return "No results found.".to_string();
        }

        results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "{}. {}\n   {}\n   {}",
                    i + 1,
                    r.title,
                    r.snippet.chars().take(200).collect::<String>(),
                    r.url
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Convert search results to JSON array
    fn results_to_json(results: &[SearchResult]) -> Vec<serde_json::Value> {
        results
            .iter()
            .map(|r| {
                json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.snippet,
                    "relevance_score": r.relevance_score,
                    "published_date": r.published_date,
                })
            })
            .collect()
    }
}

#[async_trait]
impl AgentTool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "web_search",
            "Search the web for real-time information, news, and facts.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query keywords"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)",
                        "default": 5
                    },
                    "language": {
                        "type": "string",
                        "description": "Language code (e.g., 'en', 'zh')"
                    },
                    "date_range": {
                        "type": "string",
                        "enum": ["day", "week", "month", "year"],
                        "description": "Filter by date range"
                    }
                },
                "required": ["query"]
            }),
            ToolCategory::Native,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        if !self.ctx.is_enabled() {
            return Ok(ToolResult::error(
                "Web search is disabled. Enable it in configuration to use this tool.",
            ));
        }

        // Parse parameters
        let params: WebSearchParams = serde_json::from_str(args).map_err(|e| {
            AetherError::InvalidConfig {
                message: format!("Invalid web_search parameters: {}", e),
                suggestion: Some("Provide a valid JSON object with 'query' field".to_string()),
            }
        })?;

        // Apply PII scrubbing if enabled
        let query = if self.ctx.config.pii_scrubbing {
            scrub_pii(&params.query)
        } else {
            params.query.clone()
        };

        // Get registry
        let guard = self.ctx.registry.lock().await;
        let registry = guard.as_ref().ok_or_else(|| {
            AetherError::other("Search registry not initialized".to_string())
        })?;

        // Build search options
        let options = SearchOptions {
            max_results: params
                .max_results
                .unwrap_or(self.ctx.config.default_max_results),
            timeout_seconds: self.ctx.config.default_timeout_seconds,
            language: params.language,
            date_range: params.date_range,
            ..Default::default()
        };

        // Execute search
        let results = registry.search(&query, &options).await?;

        let content = Self::format_results(&results);
        let result_json = Self::results_to_json(&results);

        Ok(ToolResult::success_with_data(
            content,
            json!({
                "query": query,
                "count": results.len(),
                "results": result_json,
            }),
        ))
    }

    fn requires_confirmation(&self) -> bool {
        false // Search is passive, no confirmation needed
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Native
    }
}

/// Simple PII scrubbing for search queries
///
/// Removes common PII patterns before sending to external search APIs.
fn scrub_pii(text: &str) -> String {
    let mut result = text.to_string();

    // Email pattern
    if let Ok(email_re) = Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}") {
        result = email_re.replace_all(&result, "[EMAIL]").to_string();
    }

    // SSN pattern (US) - more specific, apply before phone
    if let Ok(ssn_re) = Regex::new(r"\d{3}-\d{2}-\d{4}") {
        result = ssn_re.replace_all(&result, "[SSN]").to_string();
    }

    // Credit card pattern - more specific, apply before phone
    if let Ok(cc_re) = Regex::new(r"\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}") {
        result = cc_re.replace_all(&result, "[CARD]").to_string();
    }

    // Phone pattern - apply last (most greedy)
    if let Ok(phone_re) = Regex::new(r"\+?[\d\s\-().]{10,}") {
        result = phone_re.replace_all(&result, "[PHONE]").to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> (WebSearchTool, SearchContext) {
        let config = SearchConfig::default();
        let ctx = SearchContext::new(config);
        let tool = WebSearchTool::new(ctx.clone());
        (tool, ctx)
    }

    fn create_disabled_tool() -> WebSearchTool {
        let config = SearchConfig::disabled();
        let ctx = SearchContext::new(config);
        WebSearchTool::new(ctx)
    }

    #[tokio::test]
    async fn test_web_search_disabled() {
        let tool = create_disabled_tool();

        let args = json!({ "query": "test" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("disabled"));
    }

    #[tokio::test]
    async fn test_web_search_no_registry() {
        let (tool, _ctx) = create_test_tool();

        let args = json!({ "query": "test" }).to_string();
        let result = tool.execute(&args).await;

        // Should fail because registry is not set
        assert!(result.is_err());
    }

    #[test]
    fn test_web_search_metadata() {
        let (tool, _ctx) = create_test_tool();

        assert_eq!(tool.name(), "web_search");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Native);
    }

    #[test]
    fn test_pii_scrubbing() {
        assert!(scrub_pii("email test@example.com here").contains("[EMAIL]"));
        assert!(scrub_pii("call 123-456-7890 now").contains("[PHONE]"));
        assert!(scrub_pii("ssn 123-45-6789").contains("[SSN]"));
        assert!(scrub_pii("card 1234-5678-9012-3456").contains("[CARD]"));
    }

    #[test]
    fn test_pii_scrubbing_preserves_normal_text() {
        let query = "rust async programming";
        assert_eq!(scrub_pii(query), query);
    }

    #[test]
    fn test_config_default() {
        let config = SearchConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_max_results, 5);
        assert!(config.pii_scrubbing);
    }

    #[test]
    fn test_format_results() {
        let results = vec![SearchResult {
            title: "Test Title".to_string(),
            url: "https://example.com".to_string(),
            snippet: "Test snippet".to_string(),
            relevance_score: Some(0.9),
            published_date: None,
            source_type: None,
            full_content: None,
            provider: None,
        }];

        let formatted = WebSearchTool::format_results(&results);
        assert!(formatted.contains("Test Title"));
        assert!(formatted.contains("https://example.com"));
    }

    #[test]
    fn test_format_results_empty() {
        let results: Vec<SearchResult> = vec![];
        let formatted = WebSearchTool::format_results(&results);
        assert!(formatted.contains("No results found"));
    }
}

//! Search Tool
//!
//! Wraps existing SearchRegistry to expose search as a System Tool.
//! This allows AI to actively decide when to search.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::SystemTool;
use crate::config::SearchToolConfig;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::search::{SearchOptions, SearchRegistry, SearchResult};

/// Search MCP service
///
/// Wraps the existing SearchRegistry to expose search as a System Tool.
/// This allows AI to actively decide when to search.
pub struct SearchService {
    /// Search registry (set after initialization)
    registry: Arc<Mutex<Option<SearchRegistry>>>,
    /// Configuration
    config: SearchToolConfig,
}

impl SearchService {
    /// Create a new SearchService
    pub fn new(config: SearchToolConfig) -> Self {
        Self {
            registry: Arc::new(Mutex::new(None)),
            config,
        }
    }

    /// Set the search registry (called after SearchRegistry is initialized)
    pub async fn set_registry(&self, registry: SearchRegistry) {
        let mut guard = self.registry.lock().await;
        *guard = Some(registry);
    }

    /// Get shared registry reference for external setup
    pub fn registry_handle(&self) -> Arc<Mutex<Option<SearchRegistry>>> {
        Arc::clone(&self.registry)
    }

    /// Convert search results to JSON array
    fn results_to_json(results: &[SearchResult]) -> Vec<Value> {
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

impl Default for SearchService {
    fn default() -> Self {
        Self::new(SearchToolConfig::default())
    }
}

#[async_trait]
impl SystemTool for SearchService {
    fn name(&self) -> &str {
        "builtin:search"
    }

    fn description(&self) -> &str {
        "Web search for real-time information"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![McpResource {
            uri: "search://providers".to_string(),
            name: "Search Providers".to_string(),
            description: Some("Available search providers".to_string()),
            mime_type: Some("application/json".to_string()),
        }])
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        match uri {
            "search://providers" => Ok(serde_json::to_string_pretty(&json!({
                "available_providers": ["tavily", "brave", "google", "bing", "searxng", "exa"],
                "note": "Actual availability depends on configuration"
            }))?),
            _ => Err(AetherError::NotFound(uri.to_string())),
        }
    }

    fn list_tools(&self) -> Vec<McpTool> {
        if !self.config.enabled {
            return vec![];
        }

        vec![
            McpTool {
                name: "web_search".to_string(),
                description: "Search the web for real-time information, news, and facts"
                    .to_string(),
                input_schema: json!({
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
                requires_confirmation: false,
            },
            McpTool {
                name: "search_providers".to_string(),
                description: "List available search providers and their status".to_string(),
                input_schema: json!({ "type": "object" }),
                requires_confirmation: false,
            },
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "web_search" => {
                let query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AetherError::invalid_config("Missing 'query' argument"))?;

                // Apply PII scrubbing (simple regex for now)
                let scrubbed_query = scrub_pii(query);

                let max_results = args
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(self.config.default_max_results);

                let language = args
                    .get("language")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let date_range = args
                    .get("date_range")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let options = SearchOptions {
                    max_results,
                    timeout_seconds: self.config.default_timeout_seconds,
                    language,
                    date_range,
                    ..Default::default()
                };

                let guard = self.registry.lock().await;
                let registry = guard.as_ref().ok_or_else(|| {
                    AetherError::other("Search registry not initialized".to_string())
                })?;

                let results = registry.search(&scrubbed_query, &options).await?;
                let result_json = Self::results_to_json(&results);

                Ok(McpToolResult::success(json!({
                    "query": scrubbed_query,
                    "count": results.len(),
                    "results": result_json,
                })))
            }

            "search_providers" => {
                // Return list of supported providers
                Ok(McpToolResult::success(json!({
                    "providers": ["tavily", "brave", "google", "bing", "searxng", "exa"],
                    "note": "Actual availability depends on configuration"
                })))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, _tool_name: &str) -> bool {
        // Web search is passive, never needs confirmation
        false
    }
}

/// Simple PII scrubbing for search queries
///
/// Removes common PII patterns before sending to external search APIs.
/// Order matters: more specific patterns (SSN, credit card) must be applied
/// before general patterns (phone) to avoid false matches.
fn scrub_pii(text: &str) -> String {
    use regex::Regex;

    let mut result = text.to_string();

    // Email pattern (specific format, apply early)
    if let Ok(email_re) = Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}") {
        result = email_re.replace_all(&result, "[EMAIL]").to_string();
    }

    // SSN pattern (US) - MUST be before phone pattern (more specific)
    if let Ok(ssn_re) = Regex::new(r"\d{3}-\d{2}-\d{4}") {
        result = ssn_re.replace_all(&result, "[SSN]").to_string();
    }

    // Credit card pattern - MUST be before phone pattern (more specific)
    if let Ok(cc_re) = Regex::new(r"\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}") {
        result = cc_re.replace_all(&result, "[CARD]").to_string();
    }

    // Phone pattern (various formats) - apply LAST (most greedy)
    if let Ok(phone_re) = Regex::new(r"\+?[\d\s\-().]{10,}") {
        result = phone_re.replace_all(&result, "[PHONE]").to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_search_without_registry() {
        let service = SearchService::default();
        let result = service
            .call_tool("web_search", json!({"query": "test"}))
            .await;
        // Should fail gracefully
        assert!(result.is_err());
    }

    #[test]
    fn test_search_requires_query() {
        let service = SearchService::default();
        let tools = service.list_tools();
        let web_search = tools.iter().find(|t| t.name == "web_search").unwrap();
        let schema = &web_search.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("query")));
    }

    #[test]
    fn test_search_no_confirmation() {
        let service = SearchService::default();
        assert!(!service.requires_confirmation("web_search"));
        assert!(!service.requires_confirmation("search_providers"));
    }

    #[test]
    fn test_tool_listing_disabled() {
        let config = SearchToolConfig {
            enabled: false,
            ..Default::default()
        };
        let service = SearchService::new(config);
        assert!(service.list_tools().is_empty());
    }

    #[test]
    fn test_pii_scrubbing() {
        assert!(scrub_pii("email test@example.com").contains("[EMAIL]"));
        assert!(scrub_pii("call 123-456-7890").contains("[PHONE]"));
        assert!(scrub_pii("ssn 123-45-6789").contains("[SSN]"));
        assert!(scrub_pii("card 1234-5678-9012-3456").contains("[CARD]"));
    }

    #[tokio::test]
    async fn test_search_providers_tool() {
        let service = SearchService::default();
        let result = service
            .call_tool("search_providers", json!({}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content["providers"].is_array());
    }
}

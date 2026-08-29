//! Web search tool with Tavily API integration
//!
//! Implements `AlephTool` trait for AI agent integration.

use async_trait::async_trait;
use reqwest::Client;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::env;
use tracing::{debug, info, warn};

use super::error::ToolError;
use crate::error::Result;
use crate::search::SearchRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for search tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results. Omit to use the operator's `[search].max_results`.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Result count used by the registry-less legacy Tavily path, mirroring the
/// `[search].max_results` schema default.
const DEFAULT_MAX_RESULTS: usize = 5;

/// Timeout for the registry-less legacy Tavily path. The provider-path
/// `reqwest::Client` is constructed once with `build_client()` (which sets
/// the per-request timeout); the legacy path's `Client::new()` has no
/// timeout, so without this a hung Tavily endpoint wedges the agent loop.
const LEGACY_FALLBACK_TIMEOUT_SECS: u64 = 10;

/// A single search result
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Output from search tool containing results and original query
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub query: String,
}

/// Tavily API response structure
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

/// A single result from Tavily API
#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

/// Web search tool using Tavily API
pub struct SearchTool {
    client: Client,
    api_key: Option<String>,
    /// Multi-provider search registry (when available, takes priority over direct Tavily)
    registry: Option<Arc<SearchRegistry>>,
    /// Per-request timeout for the legacy Tavily fallback branch (in seconds).
    /// The provider-path `reqwest::Client` carries its own timeout via
    /// `build_client`; this is for the bare `Client::new()` legacy path.
    fallback_timeout: std::time::Duration,
}

impl SearchTool {
    /// Tool identifier
    pub const NAME: &'static str = "search";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Search the internet for current information. Use for questions requiring up-to-date data.";

    /// Create a new `SearchTool` instance
    ///
    /// Reads `TAVILY_API_KEY` from environment variable
    pub fn new() -> Self {
        let api_key = env::var("TAVILY_API_KEY").ok();
        if api_key.is_none() {
            warn!("TAVILY_API_KEY not set - search tool will not function");
        }
        Self {
            client: Client::new(),
            api_key,
            registry: None,
            fallback_timeout: std::time::Duration::from_secs(LEGACY_FALLBACK_TIMEOUT_SECS),
        }
    }

    /// Create a new `SearchTool` instance with explicit API key
    ///
    /// Falls back to `TAVILY_API_KEY` environment variable if `api_key` is None
    pub fn with_api_key(api_key: Option<String>) -> Self {
        let resolved_key = api_key.or_else(|| env::var("TAVILY_API_KEY").ok());
        if resolved_key.is_none() {
            warn!(
                "TAVILY_API_KEY not set (neither config nor env) - search tool will not function"
            );
        } else {
            info!("SearchTool initialized with API key");
        }
        Self {
            client: Client::new(),
            api_key: resolved_key,
            registry: None,
            fallback_timeout: std::time::Duration::from_secs(LEGACY_FALLBACK_TIMEOUT_SECS),
        }
    }

    /// Create with a `SearchRegistry` for multi-provider support
    pub fn with_registry(registry: Arc<SearchRegistry>) -> Self {
        info!("SearchTool initialized with multi-provider registry");
        Self {
            client: Client::new(),
            api_key: None,
            registry: Some(registry),
            fallback_timeout: std::time::Duration::from_secs(LEGACY_FALLBACK_TIMEOUT_SECS),
        }
    }

    /// Execute a web search, trying registry first then falling back to direct Tavily API
    async fn call_impl(&self, args: SearchArgs) -> std::result::Result<SearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let args_summary = format!("搜索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        // Try SearchRegistry first (multi-provider with fallback)
        if let Some(ref registry) = self.registry {
            // Start from the operator's `[search]` defaults (max_results /
            // timeout_seconds); an explicit `limit` from the model still wins.
            let mut options = registry.default_options();
            if let Some(limit) = args.limit {
                options.max_results = limit;
            }

            match registry.search(&args.query, &options).await {
                Ok(results) => {
                    let results: Vec<SearchResult> = results
                        .into_iter()
                        .map(|r| SearchResult {
                            title: r.title,
                            url: r.url,
                            snippet: r.snippet,
                        })
                        .collect();

                    info!(count = results.len(), "Search completed via registry");
                    let result_summary = format!("找到 {} 条搜索结果", results.len());
                    notify_tool_result(Self::NAME, &result_summary, true);

                    return Ok(SearchOutput {
                        results,
                        query: args.query,
                    });
                }
                Err(e) => {
                    warn!(
                        "Registry search failed, falling back to direct Tavily: {}",
                        e
                    );
                    // Fall through to direct Tavily path
                }
            }
        }

        // Fallback: Direct Tavily API call (legacy path). No registry here, so
        // no `[search]` block was parsed — use the same default the config
        // schema documents.
        let limit = args.limit.unwrap_or(DEFAULT_MAX_RESULTS);
        let api_key = self.api_key.as_ref().ok_or_else(|| {
            notify_tool_result(Self::NAME, "No search provider available", false);
            ToolError::InvalidArgs(
                "No search provider configured (no registry and no TAVILY_API_KEY)".to_string(),
            )
        })?;

        info!(query = %args.query, limit, "Executing Tavily search");

        // Build Tavily API request
        let request_body = serde_json::json!({
            "api_key": api_key,
            "query": args.query,
            "max_results": limit,
            "include_answer": false
        });

        debug!("Sending request to Tavily API");

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .timeout(self.fallback_timeout)
            .send()
            .await
            .map_err(|e| ToolError::Network(format!("Failed to send request: {e}")))?;

        // Check response status
        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            let error_msg = format!("Tavily API returned status {status}: {error_text}");
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Execution(error_msg));
        }

        // Parse response
        let tavily_response: TavilyResponse = response.json().await.map_err(|e| {
            let error_msg = format!("Failed to parse response: {e}");
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::Execution(error_msg)
        })?;

        // Convert to our SearchResult format
        let results: Vec<SearchResult> = tavily_response
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
            })
            .collect();

        info!(count = results.len(), "Search completed successfully");

        // Notify success
        let result_summary = format!("找到 {} 条搜索结果", results.len());
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(SearchOutput {
            results,
            query: args.query,
        })
    }
}

impl Default for SearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SearchTool {
    fn clone(&self) -> Self {
        Self {
            client: Client::new(),
            api_key: self.api_key.clone(),
            registry: self.registry.clone(),
            fallback_timeout: self.fallback_timeout,
        }
    }
}

/// Implementation of `AlephTool` trait for `SearchTool`
///
/// This allows `SearchTool` to be used with Aleph's unified tool system.
#[async_trait]
impl AlephTool for SearchTool {
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = SearchArgs;
    type Output = SearchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Delegate to the internal implementation, converting ToolError to AlephError
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_limit_omitted_defers_to_config() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(
            args.limit, None,
            "an omitted limit must stay None so `[search].max_results` applies"
        );
    }

    #[test]
    fn test_search_args_with_limit() {
        let args: SearchArgs =
            serde_json::from_str(r#"{"query": "rust programming", "limit": 10}"#).unwrap();
        assert_eq!(args.query, "rust programming");
        assert_eq!(args.limit, Some(10));
    }

    #[test]
    fn test_search_tool_creation() {
        assert_eq!(SearchTool::NAME, "search");
        assert!(!SearchTool::DESCRIPTION.is_empty());

        let tool = SearchTool::new();
        // API key may or may not be set in test environment
        // Just verify the tool can be created
        assert!(tool.api_key.is_none() || tool.api_key.is_some());
    }

    #[tokio::test]
    async fn test_search_without_api_key() {
        // Temporarily clear the API key if set
        let original_key = env::var("TAVILY_API_KEY").ok();
        env::remove_var("TAVILY_API_KEY");

        let tool = SearchTool::new();
        let args = SearchArgs {
            query: "test query".to_string(),
            limit: Some(5),
        };

        // Use fully qualified syntax to avoid ambiguity with blanket impl
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());

        // Error is now AlephError (converted from ToolError)
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("TAVILY_API_KEY"),
            "Error message should contain 'TAVILY_API_KEY': {}",
            err_msg
        );

        // Restore original key if it existed
        if let Some(key) = original_key {
            env::set_var("TAVILY_API_KEY", key);
        }
    }

    #[test]
    fn test_search_tool_with_registry() {
        use crate::search::SearchRegistry;
        use crate::sync_primitives::Arc;

        let registry = Arc::new(SearchRegistry::new("tavily".to_string()));
        let tool = SearchTool::with_registry(registry);
        assert!(tool.registry.is_some());
        assert!(tool.api_key.is_none());
    }
}

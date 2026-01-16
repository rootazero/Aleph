//! Web search tool with Tavily API integration
//!
//! Implements rig's Tool trait for AI agent integration.

use reqwest::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use tracing::{debug, info, warn};

use super::error::ToolError;

/// Arguments for search tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results (default 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

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
}

impl SearchTool {
    /// Tool identifier
    pub const NAME: &'static str = "search";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Search the internet for current information. Use for questions requiring up-to-date data.";

    /// Create a new SearchTool instance
    ///
    /// Reads TAVILY_API_KEY from environment variable
    pub fn new() -> Self {
        let api_key = env::var("TAVILY_API_KEY").ok();
        if api_key.is_none() {
            warn!("TAVILY_API_KEY not set - search tool will not function");
        }
        Self {
            client: Client::new(),
            api_key,
        }
    }

    /// Create a new SearchTool instance with explicit API key
    ///
    /// Falls back to TAVILY_API_KEY environment variable if api_key is None
    pub fn with_api_key(api_key: Option<String>) -> Self {
        let resolved_key = api_key.or_else(|| env::var("TAVILY_API_KEY").ok());
        if resolved_key.is_none() {
            warn!("TAVILY_API_KEY not set (neither config nor env) - search tool will not function");
        } else {
            info!("SearchTool initialized with API key");
        }
        Self {
            client: Client::new(),
            api_key: resolved_key,
        }
    }

    /// Execute a web search using Tavily API
    ///
    /// # Arguments
    /// * `args` - Search arguments including query and limit
    ///
    /// # Returns
    /// * `Ok(SearchOutput)` - Search results with original query
    /// * `Err(ToolError)` - If API key missing or request fails
    pub async fn call(&self, args: SearchArgs) -> Result<SearchOutput, ToolError> {
        let api_key = self.api_key.as_ref().ok_or_else(|| {
            ToolError::InvalidArgs("TAVILY_API_KEY not set".to_string())
        })?;

        info!(query = %args.query, limit = args.limit, "Executing Tavily search");

        // Build Tavily API request
        let request_body = serde_json::json!({
            "api_key": api_key,
            "query": args.query,
            "max_results": args.limit,
            "include_answer": false
        });

        debug!("Sending request to Tavily API");

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ToolError::Network(format!("Failed to send request: {}", e)))?;

        // Check response status
        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ToolError::Execution(format!(
                "Tavily API returned status {}: {}",
                status, error_text
            )));
        }

        // Parse response
        let tavily_response: TavilyResponse = response
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to parse response: {}", e)))?;

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
        Self::new()
    }
}

/// Implementation of rig's Tool trait for SearchTool
///
/// This allows SearchTool to be used with rig agents via the `.tool()` method.
impl Tool for SearchTool {
    const NAME: &'static str = "search";

    type Error = ToolError;
    type Args = SearchArgs;
    type Output = SearchOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        // Use schemars to generate JSON Schema for the arguments
        let schema = schema_for!(SearchArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            // Fallback to manually defined schema if generation fails
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 5)",
                        "default": 5
                    }
                },
                "required": ["query"]
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
        SearchTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_default_limit() {
        let args: SearchArgs = serde_json::from_str(r#"{"query": "test"}"#).unwrap();
        assert_eq!(args.limit, 5);
    }

    #[test]
    fn test_search_args_with_limit() {
        let args: SearchArgs =
            serde_json::from_str(r#"{"query": "rust programming", "limit": 10}"#).unwrap();
        assert_eq!(args.query, "rust programming");
        assert_eq!(args.limit, 10);
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
            limit: 5,
        };

        let result = tool.call(args).await;
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("TAVILY_API_KEY"));
        } else {
            panic!("Expected InvalidArgs error");
        }

        // Restore original key if it existed
        if let Some(key) = original_key {
            env::set_var("TAVILY_API_KEY", key);
        }
    }
}

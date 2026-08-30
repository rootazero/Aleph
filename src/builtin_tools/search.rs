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
use crate::search::notes::snippets_clamped;
use crate::search::{Recency, SearchOptions, SearchRegistry};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::utils::text_format::truncate_chars;

/// Arguments for search tool
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
    /// Max results. Omit to use the operator's `[search].max_results`.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How fresh results have to be. Omit for no constraint.
    ///
    /// Backends with no freshness parameter are ranked behind those that have
    /// one; if none can express it the search still runs and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<Recency>,
    /// Only return results from these domains, e.g. `["docs.rs"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    /// Drop results from these domains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,
    /// Return page bodies, not just snippets. Costs far more context; use it
    /// when the answer is in the page rather than in the summary of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<bool>,
    /// Ask exactly this backend instead of the configured chain. Naming one
    /// that is not configured fails rather than answering from another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchArgs {
    /// Overlay the arguments the model actually gave onto the operator's
    /// `[search]` defaults.
    ///
    /// Only what was given: an omitted parameter has to mean "whatever the
    /// operator configured", never a default this file invented. That is why
    /// every field but `query` is an `Option` or a list — the two states
    /// ("unset" and "set to the same value as the default") are different
    /// requests once the operator changes the default.
    #[must_use]
    pub fn to_options(&self, base: &SearchOptions) -> SearchOptions {
        let mut options = base.clone();
        if let Some(limit) = self.limit {
            options.max_results = limit;
        }
        if let Some(recency) = self.recency {
            options.recency = Some(recency);
        }
        if !self.domains.is_empty() {
            options.include_domains.clone_from(&self.domains);
        }
        if !self.exclude_domains.is_empty() {
            options.exclude_domains.clone_from(&self.exclude_domains);
        }
        if let Some(full_content) = self.full_content {
            options.include_full_content = full_content;
        }
        if let Some(provider) = &self.provider {
            options.provider = Some(provider.clone());
        }
        options
    }
}

/// Result count used by the registry-less legacy Tavily path, mirroring the
/// `[search].max_results` schema default.
const DEFAULT_MAX_RESULTS: usize = 5;

/// What the legacy direct-to-Tavily path answers under.
///
/// It is not a registered provider — there is no registry on that path at all
/// — but `provider_used` must still name who answered, and "tavily" reaching
/// the model from two different code paths is the same fact either way.
const LEGACY_TAVILY_PROVIDER: &str = "tavily";

/// Longest snippet handed to the model, in characters.
///
/// Deliberately not `grep`'s 240: a grep line is a *locator* for a file the
/// model can then read, so cutting it costs a little context and nothing else.
/// A snippet is the answer itself — cut it too short and the search has to be
/// run again, which costs a round trip and another provider call. 600 fits a
/// paragraph, which is what a search backend actually returns.
const SNIPPET_MAX_CHARS: usize = 600;

/// Longest page body handed to the model per result, in characters.
///
/// `full_content` is the one parameter that can make a single `search` carry
/// more than a `web_fetch`, because it returns N bodies rather than one. The
/// per-body bound keeps the comparison to "a few pages", and the overall
/// budget is capped again by [`SearchTool::max_result_tokens`].
const FULL_CONTENT_MAX_CHARS: usize = 20_000;

/// Timeout for the registry-less legacy Tavily path. The provider-path
/// `reqwest::Client` is constructed once with `build_client()` (which sets
/// the per-request timeout); the legacy path's `Client::new()` has no
/// timeout, so without this a hung Tavily endpoint wedges the agent loop.
const LEGACY_FALLBACK_TIMEOUT_SECS: u64 = 10;

/// A single search result
///
/// The three optional fields are omitted when the backend did not report
/// them, which is not the same as reporting nothing: an absent
/// `published_date` means *this backend does not say*, and inventing one
/// would be worse than leaving the reader to ask another backend.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,
}

/// Output from search tool containing results and original query
#[derive(Debug, Clone, Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub query: String,
    /// Which backend answered. The chain can fall through several, and "who
    /// said this" is not recoverable from the results.
    pub provider_used: String,
    /// What this answer is missing and which lever gets it — a dimension the
    /// backend could not express, failures it answered after, a clamp that
    /// fired. Empty in the ordinary case, and omitted from the wire then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Map registry results onto the tool face, bounding what a single call can
/// spend, and saying so whenever it spends the bound.
///
/// A clamp nobody announces reads exactly like a page that was short: the
/// model cannot tell "this is all there is" from "there is more, fetch the
/// url". Both bounds therefore return a note rather than quietly cutting.
fn render_results(results: Vec<crate::search::SearchResult>) -> (Vec<SearchResult>, Vec<String>) {
    let mut clamped_snippets = 0usize;
    let mut clamped_bodies = 0usize;
    let mapped = results
        .into_iter()
        .map(|r| {
            let snippet = if r.snippet.chars().count() > SNIPPET_MAX_CHARS {
                clamped_snippets += 1;
                truncate_chars(&r.snippet, SNIPPET_MAX_CHARS).to_string()
            } else {
                r.snippet
            };
            let full_content = r.full_content.map(|body| {
                if body.chars().count() > FULL_CONTENT_MAX_CHARS {
                    clamped_bodies += 1;
                    truncate_chars(&body, FULL_CONTENT_MAX_CHARS).to_string()
                } else {
                    body
                }
            });
            SearchResult {
                title: r.title,
                url: r.url,
                snippet,
                relevance_score: r.relevance_score,
                published_date: r.published_date,
                full_content,
            }
        })
        .collect();

    let mut notes = Vec::new();
    if clamped_snippets > 0 {
        notes.push(snippets_clamped(clamped_snippets, SNIPPET_MAX_CHARS));
    }
    if clamped_bodies > 0 {
        notes.push(crate::search::notes::full_content_truncated(
            clamped_bodies,
            FULL_CONTENT_MAX_CHARS,
        ));
    }
    (mapped, notes)
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
            // timeout_seconds); whatever the model named still wins.
            let options = args.to_options(&registry.default_options());

            match registry.search(&args.query, &options).await {
                Ok(answer) => {
                    let (results, clamp_notes) = render_results(answer.results);
                    // The registry's notes first: which backend answered and
                    // what it could not express frames everything below it.
                    let mut notes = answer.notes;
                    notes.extend(clamp_notes);

                    info!(count = results.len(), "Search completed via registry");
                    let result_summary = format!("找到 {} 条搜索结果", results.len());
                    notify_tool_result(Self::NAME, &result_summary, true);

                    return Ok(SearchOutput {
                        results,
                        query: args.query,
                        provider_used: answer.provider,
                        notes,
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

        // Convert to our SearchResult format. Same renderer as the registry
        // path: two mappings of the same shape drift, and this one is where a
        // clamp would go unannounced.
        let (results, notes) = render_results(
            tavily_response
                .results
                .into_iter()
                .map(|r| crate::search::SearchResult::new(r.title, r.url, r.content))
                .collect(),
        );

        info!(count = results.len(), "Search completed successfully");

        // Notify success
        let result_summary = format!("找到 {} 条搜索结果", results.len());
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(SearchOutput {
            results,
            query: args.query,
            provider_used: LEGACY_TAVILY_PROVIDER.to_string(),
            notes,
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

    /// Above the global default, below `web_fetch`'s page budget per result:
    /// with `full_content` set this call carries N page bodies where a fetch
    /// carries one, so the ceiling is on the call, not on the page.
    fn max_result_tokens(&self) -> Option<usize> {
        Some(8_000)
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
            ..Default::default()
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

    /// Seven fields on `SearchOptions`, two of which had a writer before this.
    /// Every one of them has a decoder in some provider downstream, so an
    /// argument that stops here is a parameter the model can name and nothing
    /// can act on.
    #[test]
    fn every_argument_reaches_search_options() {
        let args: SearchArgs = serde_json::from_value(serde_json::json!({
            "query": "q",
            "limit": 7,
            "recency": "week",
            "domains": ["github.com"],
            "exclude_domains": ["pinterest.com"],
            "full_content": true,
            "provider": "tavily"
        }))
        .unwrap();
        let o = args.to_options(&SearchOptions::default());
        assert_eq!(o.max_results, 7);
        assert_eq!(o.recency, Some(crate::search::Recency::Week));
        assert_eq!(o.include_domains, vec!["github.com".to_string()]);
        assert_eq!(o.exclude_domains, vec!["pinterest.com".to_string()]);
        assert!(o.include_full_content);
        assert_eq!(o.provider.as_deref(), Some("tavily"));
    }

    /// The operator's `[search]` defaults apply to whatever the model omitted —
    /// omitting a parameter must not silently mean "the hardcoded default".
    #[test]
    fn omitted_arguments_defer_to_the_operator_defaults() {
        let args: SearchArgs = serde_json::from_value(serde_json::json!({"query": "q"})).unwrap();
        let base = SearchOptions {
            max_results: 11,
            timeout_seconds: 42,
            ..Default::default()
        };
        let o = args.to_options(&base);
        assert_eq!(o.max_results, 11);
        assert_eq!(o.timeout_seconds, 42);
        assert_eq!(o.recency, None);
        assert!(o.include_domains.is_empty());
    }

    /// A snippet is content, not a locator (grep clamps a line to 240 because a
    /// grep line points at a file you can then read; a snippet is the answer).
    /// It still needs a bound, and exceeding it has to be said out loud.
    #[test]
    fn long_snippets_are_clamped_and_the_clamp_is_announced() {
        let long = "x".repeat(SNIPPET_MAX_CHARS + 500);
        let (results, notes) =
            render_results(vec![crate::search::SearchResult::new("t", "u", long)]);
        assert!(results[0].snippet.chars().count() <= SNIPPET_MAX_CHARS);
        assert!(notes.iter().any(|n| n.contains("clamp")), "{notes:?}");
    }

    /// The three fields the old mapping dropped on the floor have to survive,
    /// and the two the backend did not send must stay absent rather than
    /// acquiring a value nobody reported.
    #[test]
    fn the_fields_a_backend_reported_survive_the_mapping() {
        let rich = crate::search::SearchResult {
            title: "t".into(),
            url: "u".into(),
            snippet: "s".into(),
            relevance_score: Some(0.5),
            full_content: Some("body".into()),
            published_date: Some("2024-01-01".into()),
            provider: Some("tavily".into()),
        };
        let (results, notes) = render_results(vec![
            rich,
            crate::search::SearchResult::new("t2", "u2", "s2"),
        ]);
        assert_eq!(results[0].relevance_score, Some(0.5));
        assert_eq!(results[0].full_content.as_deref(), Some("body"));
        assert_eq!(results[0].published_date.as_deref(), Some("2024-01-01"));
        assert_eq!(
            results[1].published_date, None,
            "absent means the backend did not say, so nothing may be invented"
        );
        assert!(notes.is_empty(), "nothing was clamped: {notes:?}");
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

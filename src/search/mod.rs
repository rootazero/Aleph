pub(crate) mod error;
mod factory;
pub mod handle;
mod health;
pub(crate) mod merge;
pub(crate) mod notes;
mod options;
mod provider;
pub mod providers;
mod registry;
mod result;
mod web_fetch_fallback;
/// Search capability implementation
///
/// This module provides real-time web search functionality for Aleph Agent,
/// enabling AI to access up-to-date information beyond training data cutoff.
///
/// # Architecture
///
/// - `SearchResult`: Unified data structure for all provider results
/// - `SearchOptions`: Configuration for search behavior
/// - `SearchProvider`: Trait abstraction for different search backends
/// - `SearchRegistry`: Factory and router for managing multiple providers
///
/// # Supported Providers
///
/// - **Tavily**: AI-optimized search (recommended default)
/// - **`SearXNG`**: Privacy-first, self-hosted
/// - **Brave**: Privacy + quality balance
/// - **Google CSE**: Comprehensive coverage
/// - **Bing**: Cost-effective
/// - **Exa.ai**: Semantic search
/// - **Firecrawl**: Search + full-content scraping
/// - **Jina**: LLM-ready, pre-ranked snippets (`s.jina.ai`)
/// - **`DuckDuckGo`**: Zero-credential HTML scrape (last-resort fallback)
///
/// # Example
///
/// ```rust,no_run
/// use alephcore::search::{SearchProvider, SearchOptions};
/// use alephcore::search::providers::TavilyProvider;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let provider = TavilyProvider::new("tvly-xxx".to_string())?;
///     let options = SearchOptions::default();
///
///     let results = provider.search("Rust programming language", &options).await?;
///
///     for result in results {
///         println!("Title: {}", result.title);
///         println!("URL: {}", result.url);
///         println!("Snippet: {}\n", result.snippet);
///     }
///
///     Ok(())
/// }
/// ```
// Re-exports
pub use factory::{ProviderFactory, ProviderFactoryRegistry};
pub use handle::{
    decline_global_search_handle, install_global_search_handle, try_global_search_handle,
    SearchApplyError, SearchHandle,
};
pub use options::{Recency, SearchOptions, MAX_SEARCH_RESULTS};
pub use provider::{SearchCapabilities, SearchProvider};
pub use registry::{MultiSearchAnswer, SearchAnswer, SearchRegistry};
pub use result::SearchResult;
pub use web_fetch_fallback::WebFetchSerpFallback;

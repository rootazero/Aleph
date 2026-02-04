mod options;
mod provider;
pub mod providers;
mod registry;
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
/// - **SearXNG**: Privacy-first, self-hosted
/// - **Brave**: Privacy + quality balance
/// - **Google CSE**: Comprehensive coverage
/// - **Bing**: Cost-effective
/// - **Exa.ai**: Semantic search
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
// Core modules
mod result;

// Re-exports
pub use options::{QuotaInfo, SearchOptions};
pub use provider::SearchProvider;
pub use registry::SearchRegistry;
pub use result::SearchResult;

/// Result of testing a search provider connection
///
/// Used by the UI to display provider status and validate configuration.
#[derive(Debug, Clone)]
pub struct ProviderTestResult {
    /// Whether the test was successful
    pub success: bool,
    /// Response time in milliseconds (0 if failed)
    pub latency_ms: u32,
    /// Error message (empty if success)
    pub error_message: String,
    /// Error type: "auth", "network", "config", or empty if success
    pub error_type: String,
}

/// Configuration for ad-hoc search provider testing
///
/// This allows testing provider credentials without saving to config file.
/// Used by the UI to validate provider settings before committing changes.
#[derive(Debug, Clone)]
pub struct SearchProviderTestConfig {
    /// Provider type: "tavily", "brave", "searxng", "google", "bing", "exa"
    pub provider_type: String,
    /// API key (required for most providers)
    pub api_key: Option<String>,
    /// Base URL (required for SearXNG)
    pub base_url: Option<String>,
    /// Engine ID (required for Google CSE)
    pub engine_id: Option<String>,
}

mod factory;
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
pub use options::{QuotaInfo, SearchOptions};
pub use provider::SearchProvider;
pub use registry::SearchRegistry;
pub use result::SearchResult;
pub use web_fetch_fallback::WebFetchSerpFallback;

/// Result of testing a search provider connection
///
/// Used by the UI to display provider status and validate configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Supported search provider types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderType {
    /// Tavily AI-optimized search
    Tavily,
    /// Brave privacy-focused search
    Brave,
    /// `SearXNG` self-hosted metasearch
    Searxng,
    /// Google Custom Search Engine
    Google,
    /// Bing Web Search
    Bing,
    /// Exa.ai semantic search
    Exa,
    /// Jina AI search
    Jina,
    /// `DuckDuckGo` HTML-scrape search
    DuckDuckGo,
    /// Firecrawl search + full-content scraping
    Firecrawl,
}

impl SearchProviderType {
    /// Returns the string identifier for this provider type
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Tavily => "tavily",
            Self::Brave => "brave",
            Self::Searxng => "searxng",
            Self::Google => "google",
            Self::Bing => "bing",
            Self::Exa => "exa",
            Self::Jina => "jina",
            Self::DuckDuckGo => "duckduckgo",
            Self::Firecrawl => "firecrawl",
        }
    }
}

impl std::fmt::Display for SearchProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for SearchProviderType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tavily" => Ok(Self::Tavily),
            "brave" => Ok(Self::Brave),
            "searxng" => Ok(Self::Searxng),
            "google" => Ok(Self::Google),
            "bing" => Ok(Self::Bing),
            "exa" => Ok(Self::Exa),
            "jina" => Ok(Self::Jina),
            "duckduckgo" => Ok(Self::DuckDuckGo),
            "firecrawl" => Ok(Self::Firecrawl),
            _ => Err(format!("Unknown provider type: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_round_trip() {
        for variant in [
            SearchProviderType::Tavily,
            SearchProviderType::Brave,
            SearchProviderType::Searxng,
            SearchProviderType::Google,
            SearchProviderType::Bing,
            SearchProviderType::Exa,
            SearchProviderType::Jina,
            SearchProviderType::DuckDuckGo,
            SearchProviderType::Firecrawl,
        ] {
            let s = variant.as_str();
            let parsed: SearchProviderType = s.parse().unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn test_provider_type_display() {
        assert_eq!(SearchProviderType::Tavily.to_string(), "tavily");
        assert_eq!(SearchProviderType::Google.to_string(), "google");
    }

    #[test]
    fn test_provider_type_rejects_unknown() {
        let result: Result<SearchProviderType, _> = "unknown".parse();
        assert!(result.is_err());
    }
}

/// Configuration for ad-hoc search provider testing
///
/// This allows testing provider credentials without saving to config file.
/// Used by the UI to validate provider settings before committing changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProviderTestConfig {
    /// Provider type
    pub provider_type: SearchProviderType,
    /// API key (required for most providers)
    pub api_key: Option<String>,
    /// Base URL (required for `SearXNG`)
    pub base_url: Option<String>,
    /// Engine ID (required for Google CSE)
    pub engine_id: Option<String>,
}

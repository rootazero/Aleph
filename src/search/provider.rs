use crate::error::Result;
use crate::search::{SearchOptions, SearchResult};
/// Search provider trait abstraction
///
/// This module defines the `SearchProvider` trait which all search backends implement
use async_trait::async_trait;

/// What a provider can express on the wire.
///
/// A bit here is a promise: the registry uses it as a **sorting key** (spec
/// §3) — a provider that claims `domain_filter` gets the requests that ask
/// for one. Claiming a parameter you do not send therefore does not fail
/// loudly, it silently widens somebody's search. `capability_census.rs`
/// compares each bit against the request builder that would have to send it.
///
/// The default is all-`false` on purpose: a new provider that forgets to
/// declare anything is invisible to dimension-aware routing, which is the
/// safe direction. The reverse default would make it claim everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchCapabilities {
    /// Accepts an include/exclude domain list.
    pub domain_filter: bool,
    /// Accepts a freshness constraint (`SearchOptions::recency`).
    pub recency: bool,
    /// Can return page bodies, not just snippets.
    pub full_content: bool,
}

/// Unified interface for search providers
///
/// All search backends (Tavily, Google, `SearXNG`, etc.) implement this trait.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Execute a search query
    ///
    /// # Arguments
    ///
    /// * `query` - Search keywords
    /// * `options` - Search options (language, region, filters, etc.)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<SearchResult>)` - List of search results
    /// * `Err(AlephError)` - Network error, API error, quota exceeded, etc.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use alephcore::search::{SearchProvider, SearchOptions};
    /// # async fn example(provider: &dyn SearchProvider) {
    /// let options = SearchOptions::default();
    /// let results = provider.search("Rust async", &options).await.unwrap();
    /// # }
    /// ```
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>>;

    /// Get provider name (for logging/debugging)
    fn name(&self) -> &str;

    /// Check if provider is configured and available
    ///
    /// Returns `false` if API key is missing or invalid
    fn is_available(&self) -> bool;

    /// What this provider can express. Default: nothing — see
    /// [`SearchCapabilities`] for why the default is not "everything".
    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;

    // Mock implementation for testing
    struct MockSearchProvider {
        name: String,
        available: bool,
    }

    #[async_trait]
    impl SearchProvider for MockSearchProvider {
        async fn search(&self, query: &str, _options: &SearchOptions) -> Result<Vec<SearchResult>> {
            Ok(vec![SearchResult::new(
                "Mock Title".to_string(),
                "https://mock.com".to_string(),
                format!("Mock result for query: {query}"),
            )])
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn is_available(&self) -> bool {
            self.available
        }
    }

    #[tokio::test]
    async fn test_mock_provider() {
        let provider = MockSearchProvider {
            name: "mock".to_string(),
            available: true,
        };

        let options = SearchOptions::default();
        let results = provider.search("test query", &options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Mock Title");
        assert!(results[0].snippet.contains("test query"));
    }

    #[tokio::test]
    async fn test_provider_metadata() {
        let provider = MockSearchProvider {
            name: "test-provider".to_string(),
            available: true,
        };

        assert_eq!(provider.name(), "test-provider");
        assert!(provider.is_available());
    }

    #[test]
    fn test_provider_is_send_sync() {
        // This test ensures SearchProvider can be used across threads
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn SearchProvider>>();
    }
}

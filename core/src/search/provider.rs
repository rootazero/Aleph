use crate::error::Result;
use crate::search::{QuotaInfo, SearchOptions, SearchResult};
/// Search provider trait abstraction
///
/// This module defines the `SearchProvider` trait which all search backends implement
use async_trait::async_trait;

/// Unified interface for search providers
///
/// All search backends (Tavily, Google, SearXNG, etc.) implement this trait
/// to provide a consistent API to the CapabilityExecutor.
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

    /// Get quota information (optional, returns unlimited by default)
    async fn get_quota(&self) -> Result<QuotaInfo> {
        Ok(QuotaInfo::unlimited())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
                format!("Mock result for query: {}", query),
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

    #[tokio::test]
    async fn test_provider_quota_default() {
        let provider = MockSearchProvider {
            name: "test".to_string(),
            available: true,
        };

        let quota = provider.get_quota().await.unwrap();
        assert!(quota.remaining.is_none());
        assert!(quota.limit.is_none());
    }

    #[test]
    fn test_provider_is_send_sync() {
        // This test ensures SearchProvider can be used across threads
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn SearchProvider>>();
    }
}

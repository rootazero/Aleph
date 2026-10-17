//! Compatibility re-export shim — the definitions moved to
//! [`aleph_slot_api::search`] in Task 3 of the P0 modularization
//! (`SearchCapabilities` with its all-`false` default, and the
//! `SearchProvider` trait with its options-aware `capabilities` default).
//!
//! This is a `pub use`, not a second definition: both paths share one
//! `TypeId`, pinned by `tests/modularization_contracts.rs`.

pub use aleph_slot_api::search::{SearchCapabilities, SearchProvider};

#[cfg(test)]
mod tests {
    //! The two `#[tokio::test]` mock-provider tests stay in Core: the API
    //! crate carries no runtime dependency, even as a dev-dependency, so an
    //! async test cannot live there. The plain `test_provider_is_send_sync`
    //! moved with the trait to `aleph_slot_api::search::tests`.
    use super::*;
    use crate::error::Result;
    use crate::search::{SearchOptions, SearchResult};
    use async_trait::async_trait;

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
}

//! Standalone API-only search consumer with local data and no network I/O.
use aleph_slot_api::{
    async_trait, Result, SearchCapabilities, SearchOptions, SearchProvider, SearchResult,
};

pub struct LocalSearchProvider;

#[async_trait]
impl SearchProvider for LocalSearchProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let mut results = vec![SearchResult::new(
            query,
            "local://search/task-10",
            "Local SearchProvider consumer fixture.",
        )];
        results.truncate(options.validated_max_results());
        Ok(results)
    }

    fn name(&self) -> &str {
        "external-local-search"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn capabilities(&self, options: &SearchOptions) -> SearchCapabilities {
        let _ = options;
        SearchCapabilities::default()
    }
}

/// An external caller supplies the executor; this crate installs no runtime.
/// These assertions are a callable witness, not a claim that it was executed.
pub async fn local_search_witness(query: &str) -> Result<Vec<SearchResult>> {
    let provider: &dyn SearchProvider = &LocalSearchProvider;
    let options = SearchOptions::default();
    let defaults = SearchCapabilities::default();
    assert!(!defaults.domain_filter);
    assert!(!defaults.recency);
    assert!(!defaults.full_content);
    assert_eq!(provider.capabilities(&options), defaults);
    assert_eq!(provider.name(), "external-local-search");
    assert!(provider.is_available());

    let results = provider.search(query, &options).await?;
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.title, query);
    assert_eq!(result.url, "local://search/task-10");
    assert_eq!(result.snippet, "Local SearchProvider consumer fixture.");
    assert_eq!(result.relevance_score, None);
    assert_eq!(result.full_content, None);
    assert_eq!(result.published_date, None);
    assert_eq!(result.provider, None);
    Ok(results)
}

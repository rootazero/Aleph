//! Search capability strategy.
//!
//! This strategy performs web searches using the configured search registry
//! and populates the payload with search results.

use crate::capability::strategy::CapabilityStrategy;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::search::{SearchOptions, SearchRegistry};
use crate::utils::pii;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Search capability strategy
///
/// Performs web searches and enriches the payload with results.
pub struct SearchStrategy {
    /// Search registry for provider management
    search_registry: Option<Arc<SearchRegistry>>,
    /// Search options (timeout, max results, etc.)
    search_options: SearchOptions,
    /// Enable PII scrubbing for search queries
    pii_scrubbing_enabled: bool,
}

impl SearchStrategy {
    /// Create a new search strategy
    pub fn new(
        search_registry: Option<Arc<SearchRegistry>>,
        search_options: Option<SearchOptions>,
        pii_scrubbing_enabled: bool,
    ) -> Self {
        Self {
            search_registry,
            search_options: search_options.unwrap_or_default(),
            pii_scrubbing_enabled,
        }
    }

    /// Update the search registry
    pub fn set_registry(&mut self, registry: Arc<SearchRegistry>) {
        self.search_registry = Some(registry);
    }

    /// Update search options
    pub fn set_options(&mut self, options: SearchOptions) {
        self.search_options = options;
    }

    /// Extract search query from user input
    ///
    /// For MVP, this is a simple pass-through. Future versions could
    /// implement more sophisticated query extraction.
    fn extract_search_query(input: &str) -> Option<String> {
        let query = input.trim();
        if query.is_empty() {
            None
        } else {
            Some(query.to_string())
        }
    }
}

#[async_trait]
impl CapabilityStrategy for SearchStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Search
    }

    fn priority(&self) -> u32 {
        1 // Search executes second (after Memory)
    }

    fn is_available(&self) -> bool {
        self.search_registry.is_some()
    }

    fn validate_config(&self) -> Result<()> {
        // Check if timeout is reasonable
        if self.search_options.timeout_seconds == 0 {
            return Err(crate::error::AetherError::config(
                "Search timeout cannot be 0",
            ));
        }

        // Check if max_results is reasonable
        if self.search_options.max_results == 0 {
            return Err(crate::error::AetherError::config(
                "Search max_results cannot be 0",
            ));
        }

        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        // Search is healthy if registry is configured
        // Actual provider availability is checked at runtime during search
        if self.search_registry.is_some() {
            debug!("Search registry configured");
        }
        Ok(self.is_available())
    }

    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert("capability".to_string(), "Search".to_string());
        info.insert("name".to_string(), "search".to_string());
        info.insert("priority".to_string(), "1".to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info.insert(
            "has_registry".to_string(),
            self.search_registry.is_some().to_string(),
        );
        info.insert(
            "timeout_seconds".to_string(),
            self.search_options.timeout_seconds.to_string(),
        );
        info.insert(
            "max_results".to_string(),
            self.search_options.max_results.to_string(),
        );
        info.insert(
            "pii_scrubbing".to_string(),
            self.pii_scrubbing_enabled.to_string(),
        );
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Check if search registry is available
        let Some(registry) = &self.search_registry else {
            warn!("Search capability requested but no search registry configured");
            return Ok(payload);
        };

        // Extract search query from user input
        let Some(mut query) = Self::extract_search_query(&payload.user_input) else {
            warn!("Search capability requested but user input is empty");
            return Ok(payload);
        };

        // Apply PII scrubbing if enabled
        if self.pii_scrubbing_enabled {
            let scrubbed = pii::scrub_pii(&query);
            if scrubbed != query {
                debug!("PII scrubbing applied to search query");
            }
            query = scrubbed;
        }

        info!(
            query_length = query.len(),
            max_results = self.search_options.max_results,
            timeout = self.search_options.timeout_seconds,
            pii_scrubbing = self.pii_scrubbing_enabled,
            "Executing search capability"
        );

        // Perform search with timeout
        let search_future = registry.search(&query, &self.search_options);
        let timeout_duration = Duration::from_secs(self.search_options.timeout_seconds);

        match tokio::time::timeout(timeout_duration, search_future).await {
            Ok(Ok(results)) => {
                if results.is_empty() {
                    info!("Search completed but no results found");
                    payload.context.search_results = None;
                } else {
                    info!(
                        count = results.len(),
                        provider = results.first().and_then(|r| r.provider.as_deref()),
                        "Search completed successfully"
                    );
                    payload.context.search_results = Some(results);
                }
            }
            Ok(Err(e)) => {
                warn!(
                    error = %e,
                    "Search failed, continuing without results"
                );
                payload.context.search_results = None;
            }
            Err(_) => {
                warn!(
                    timeout = self.search_options.timeout_seconds,
                    "Search timed out, continuing without results"
                );
                payload.context.search_results = None;
            }
        }

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};
    use crate::search::SearchResult;

    /// Mock search provider for testing
    struct MockSearchProvider {
        name: String,
        results: Vec<SearchResult>,
    }

    impl MockSearchProvider {
        fn new(name: &str, result_count: usize) -> Self {
            let results = (0..result_count)
                .map(|i| SearchResult {
                    title: format!("Test Result {}", i + 1),
                    url: format!("https://test.com/{}", i + 1),
                    snippet: format!("Test snippet {}", i + 1),
                    full_content: None,
                    source_type: None,
                    provider: Some(name.to_string()),
                    published_date: None,
                    relevance_score: Some(0.9 - (i as f32 * 0.1)),
                })
                .collect();
            Self {
                name: name.to_string(),
                results,
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::search::SearchProvider for MockSearchProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn search(
            &self,
            _query: &str,
            _options: &SearchOptions,
        ) -> Result<Vec<SearchResult>> {
            Ok(self.results.clone())
        }
    }

    #[tokio::test]
    async fn test_search_strategy_not_available() {
        let strategy = SearchStrategy::new(None, None, false);
        assert!(!strategy.is_available());
    }

    #[tokio::test]
    async fn test_search_strategy_execute_no_registry() {
        let strategy = SearchStrategy::new(None, None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test query".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.search_results.is_none());
    }

    #[tokio::test]
    async fn test_search_strategy_with_mock_registry() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 3);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let strategy = SearchStrategy::new(Some(Arc::new(registry)), None, false);
        assert!(strategy.is_available());

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("Test query".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.search_results.is_some());
        assert_eq!(result.context.search_results.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_search_strategy_empty_query() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockSearchProvider::new("mock", 1);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let strategy = SearchStrategy::new(Some(Arc::new(registry)), None, false);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Search],
                ContextFormat::Markdown,
            )
            .user_input("   ".to_string()) // Empty after trim
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.search_results.is_none());
    }
}

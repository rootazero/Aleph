use crate::error::{AlephError, Result};
use crate::search::{ProviderTestResult, SearchOptions, SearchProvider, SearchResult};
use crate::sync_primitives::{Arc, Mutex};
/// Search provider registry and router
///
/// This module manages multiple search providers and routes requests
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Registry for managing multiple search providers
///
/// Maintains a pool of configured providers and routes search requests
/// to the appropriate backend based on configuration.
pub struct SearchRegistry {
    providers: HashMap<String, Arc<dyn SearchProvider>>,
    default_provider: String,
    fallback_providers: Vec<String>,
    /// Cache for provider test results (name -> (result, timestamp))
    test_cache: Arc<Mutex<HashMap<String, (ProviderTestResult, Instant)>>>,
}

impl SearchRegistry {
    /// Create an empty registry
    pub fn new(default_provider: impl Into<String>) -> Self {
        Self {
            providers: HashMap::new(),
            default_provider: default_provider.into(),
            fallback_providers: Vec::new(),
            test_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Add a provider to the registry
    ///
    /// Invalidates any cached test results for this provider name
    /// to ensure stale results are not returned after configuration changes.
    pub fn add_provider(&mut self, name: String, provider: Arc<dyn SearchProvider>) {
        self.providers.insert(name.clone(), provider);
        // Invalidate cached test result for this provider
        let mut cache = self.test_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.remove(&name);
    }

    /// Set fallback providers
    pub fn set_fallback_providers(&mut self, providers: Vec<String>) {
        self.fallback_providers = providers;
    }

    /// Get a provider by name
    pub fn get_provider(&self, name: &str) -> Option<&Arc<dyn SearchProvider>> {
        self.providers.get(name)
    }

    /// Execute search with fallback logic
    ///
    /// Tries default provider first, then falls back to alternatives if it fails.
    /// Aggregates error messages from all attempted providers.
    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let mut errors: Vec<String> = Vec::new();

        // Try default provider
        if let Some(provider) = self.providers.get(&self.default_provider) {
            if !provider.is_available() {
                let msg = format!(
                    "Default provider '{}' is not available (missing configuration)",
                    self.default_provider
                );
                log::warn!("{}", msg);
                errors.push(msg);
            } else {
                match provider.search(query, options).await {
                    Ok(results) => return Ok(results),
                    Err(e) => {
                        let msg = format!("Provider '{}' failed: {}", self.default_provider, e);
                        log::warn!("Search {}", msg);
                        errors.push(msg);
                    }
                }
            }
        } else {
            let msg = format!("Default provider '{}' not found", self.default_provider);
            log::warn!("{}", msg);
            errors.push(msg);
        }

        // Try fallback providers
        for provider_name in &self.fallback_providers {
            if let Some(provider) = self.providers.get(provider_name) {
                if !provider.is_available() {
                    let msg = format!(
                        "Fallback provider '{}' is not available (missing configuration)",
                        provider_name
                    );
                    log::warn!("{}", msg);
                    errors.push(msg);
                    continue;
                }
                match provider.search(query, options).await {
                    Ok(results) => {
                        log::info!(
                            "Search succeeded with fallback provider '{}'",
                            provider_name
                        );
                        return Ok(results);
                    }
                    Err(e) => {
                        let msg = format!("Provider '{}' failed: {}", provider_name, e);
                        log::warn!("Search {}", msg);
                        errors.push(msg);
                    }
                }
            }
        }

        let summary = if errors.is_empty() {
            "All search providers failed".to_string()
        } else {
            format!("All search providers failed: {}", errors.join("; "))
        };
        Err(AlephError::provider(summary))
    }

    /// Test a search provider connection
    ///
    /// Executes a minimal test query to validate API key and connectivity.
    /// Results are cached for 5 minutes to avoid excessive API calls.
    ///
    /// # Arguments
    /// * `name` - Provider name to test
    ///
    /// # Returns
    /// * `ProviderTestResult` - Test result with latency or error information
    pub async fn test_search_provider(&self, name: &str) -> ProviderTestResult {
        const CACHE_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes

        // Check cache first
        {
            let cache = self.test_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((result, timestamp)) = cache.get(name) {
                if timestamp.elapsed() < CACHE_TTL {
                    log::debug!("Returning cached test result for provider '{}'", name);
                    return result.clone();
                }
            }
        }

        // Provider not found
        let provider = match self.providers.get(name) {
            Some(p) => p,
            None => {
                let result = ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message: format!("Provider '{}' not found in registry", name),
                    error_type: "config".to_string(),
                };
                return result;
            }
        };

        // Skip test if provider is not configured
        if !provider.is_available() {
            return ProviderTestResult {
                success: false,
                latency_ms: 0,
                error_message: format!("Provider '{}' is not configured (missing API key)", name),
                error_type: "config".to_string(),
            };
        }

        // Execute test search
        let test_options = SearchOptions {
            max_results: 1,
            timeout_seconds: 5,
            ..Default::default()
        };

        let start = Instant::now();
        let result = match provider.search("test", &test_options).await {
            Ok(_) => {
                let latency = start.elapsed().as_millis().min(u32::MAX as u128) as u32;
                ProviderTestResult {
                    success: true,
                    latency_ms: latency,
                    error_message: String::new(),
                    error_type: String::new(),
                }
            }
            Err(e) => {
                let error_type = match e {
                    AlephError::AuthenticationError { .. } => "auth",
                    AlephError::NetworkError { .. } | AlephError::Timeout { .. } => "network",
                    _ => "config",
                };

                ProviderTestResult {
                    success: false,
                    latency_ms: 0,
                    error_message: e.to_string(),
                    error_type: error_type.to_string(),
                }
            }
        };

        // Cache only successful results; failures are retried on next call
        if result.success {
            let mut cache = self.test_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.insert(name.to_string(), (result.clone(), Instant::now()));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock provider for testing
    struct MockProvider {
        name: String,
        should_fail: bool,
        result_count: usize,
    }

    impl MockProvider {
        fn new(name: &str, should_fail: bool, result_count: usize) -> Self {
            Self {
                name: name.to_string(),
                should_fail,
                result_count,
            }
        }
    }

    #[async_trait::async_trait]
    impl SearchProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
            if self.should_fail {
                return Err(AlephError::network("Mock provider failure"));
            }

            let mut results = Vec::new();
            for i in 0..self.result_count.min(options.max_results) {
                results.push(SearchResult {
                    title: format!("{} - Result {}", query, i + 1),
                    url: format!("https://example.com/{}", i + 1),
                    snippet: format!("Snippet for result {}", i + 1),
                    full_content: None,
                    source_type: None,
                    provider: Some(self.name.clone()),
                    published_date: None,
                    relevance_score: Some(1.0 - (i as f32 * 0.1)),
                });
            }
            Ok(results)
        }
    }

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = SearchRegistry::new("tavily".to_string());
        assert_eq!(registry.default_provider, "tavily");
        assert!(registry.providers.is_empty());
    }

    #[tokio::test]
    async fn test_registry_add_provider() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockProvider::new("mock", false, 3);

        registry.add_provider("mock".to_string(), Arc::new(provider));

        assert!(registry.get_provider("mock").is_some());
    }

    #[tokio::test]
    async fn test_registry_search_with_mock_provider() {
        let mut registry = SearchRegistry::new("mock".to_string());
        let provider = MockProvider::new("mock", false, 5);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let options = SearchOptions {
            max_results: 3,
            timeout_seconds: 5,
            ..Default::default()
        };

        let results = registry.search("test query", &options).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "test query - Result 1");
        assert_eq!(results[0].provider, Some("mock".to_string()));
    }

    #[tokio::test]
    async fn test_registry_search_no_provider() {
        let registry = SearchRegistry::new("nonexistent".to_string());
        let options = SearchOptions::default();

        let result = registry.search("test", &options).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("All search providers failed"));
    }

    #[tokio::test]
    async fn test_registry_fallback_to_default() {
        let mut registry = SearchRegistry::new("default".to_string());

        // Add default provider that succeeds
        let default_provider = MockProvider::new("default", false, 3);
        registry.add_provider("default".to_string(), Arc::new(default_provider));

        // Set fallback to nonexistent provider
        registry.set_fallback_providers(vec!["nonexistent".to_string()]);

        let options = SearchOptions::default();
        let results = registry.search("test", &options).await.unwrap();

        // Should get results from default provider
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].provider, Some("default".to_string()));
    }

    #[tokio::test]
    async fn test_registry_fallback_chain() {
        let mut registry = SearchRegistry::new("primary".to_string());

        // Primary provider fails
        let primary = MockProvider::new("primary", true, 0);
        registry.add_provider("primary".to_string(), Arc::new(primary));

        // First fallback fails
        let fallback1 = MockProvider::new("fallback1", true, 0);
        registry.add_provider("fallback1".to_string(), Arc::new(fallback1));

        // Second fallback succeeds
        let fallback2 = MockProvider::new("fallback2", false, 2);
        registry.add_provider("fallback2".to_string(), Arc::new(fallback2));

        registry.set_fallback_providers(vec!["fallback1".to_string(), "fallback2".to_string()]);

        let options = SearchOptions::default();
        let results = registry.search("test", &options).await.unwrap();

        // Should get results from second fallback
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider, Some("fallback2".to_string()));
    }

    #[tokio::test]
    async fn test_registry_all_providers_fail() {
        let mut registry = SearchRegistry::new("primary".to_string());

        // All providers fail
        let primary = MockProvider::new("primary", true, 0);
        registry.add_provider("primary".to_string(), Arc::new(primary));

        let fallback = MockProvider::new("fallback", true, 0);
        registry.add_provider("fallback".to_string(), Arc::new(fallback));

        registry.set_fallback_providers(vec!["fallback".to_string()]);

        let options = SearchOptions::default();
        let result = registry.search("test", &options).await;

        // Should fail when all providers fail
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_registry_respects_max_results() {
        let mut registry = SearchRegistry::new("mock".to_string());

        // Provider can return 10 results
        let provider = MockProvider::new("mock", false, 10);
        registry.add_provider("mock".to_string(), Arc::new(provider));

        let options = SearchOptions {
            max_results: 5,
            timeout_seconds: 5,
            ..Default::default()
        };

        let results = registry.search("test", &options).await.unwrap();

        // Should only get max_results
        assert_eq!(results.len(), 5);
    }
}

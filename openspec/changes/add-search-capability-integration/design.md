# Design Document: Search Capability Integration

**Change ID**: add-search-capability-integration
**Date**: 2026-01-04

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Data Structures](#data-structures)
3. [Provider Abstraction Layer](#provider-abstraction-layer)
4. [Provider Implementations](#provider-implementations)
5. [Configuration Schema](#configuration-schema)
6. [Integration Flow](#integration-flow)
7. [Error Handling Strategy](#error-handling-strategy)
8. [Security & Privacy](#security--privacy)
9. [Performance Considerations](#performance-considerations)
10. [Testing Approach](#testing-approach)

---

## Architecture Overview

### Layered Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    macOS Client (SwiftUI)                    │
│                    [No UI changes in MVP]                    │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼ UniFFI
┌─────────────────────────────────────────────────────────────┐
│                      Rust Core Library                       │
├─────────────────────────────────────────────────────────────┤
│  ┌───────────────────────────────────────────────────────┐  │
│  │            CapabilityExecutor::execute_search()        │  │
│  │  - Reads AgentPayload.capabilities                     │  │
│  │  - Extracts search query from user input               │  │
│  │  - Calls SearchRegistry.search()                       │  │
│  │  - Fills AgentContext.search_results                   │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                               │
│                              ▼                               │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                   SearchRegistry                       │  │
│  │  - Maintains HashMap<String, Box<dyn SearchProvider>>  │  │
│  │  - Routes to configured provider (e.g., "tavily")      │  │
│  │  - Implements fallback logic on error                  │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                               │
│                              ▼                               │
│  ┌───────────────────────────────────────────────────────┐  │
│  │          SearchProvider Trait (Abstraction)            │  │
│  │  async fn search(&self, query, options) -> Result<..>  │  │
│  │  fn name(&self) -> &str                                │  │
│  │  fn is_available(&self) -> bool                        │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                               │
│         ┌────────────────────┼────────────────────┐          │
│         ▼                    ▼                    ▼          │
│  ┌───────────┐      ┌───────────┐        ┌───────────┐      │
│  │  Tavily   │      │ SearXNG   │  ...   │  Brave    │      │
│  │ Provider  │      │ Provider  │        │ Provider  │      │
│  └───────────┘      └───────────┘        └───────────┘      │
│         │                    │                    │          │
│         └────────────────────┴────────────────────┘          │
│                              │                               │
│                              ▼ HTTP (reqwest)                │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              External Search APIs                      │  │
│  │  - Tavily: https://api.tavily.com/search              │  │
│  │  - SearXNG: http://localhost:8080/search?format=json  │  │
│  │  - Brave: https://api.search.brave.com/res/v1/...     │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Key Design Principles

1. **Dependency Inversion**: Upper layers depend on `SearchProvider` trait, not concrete implementations
2. **Plugin Architecture**: New providers can be added without modifying existing code
3. **Fail-Safe Design**: Search failures do not crash the application
4. **Privacy by Default**: PII scrubbing happens before query reaches network layer
5. **Async-First**: All operations are non-blocking (tokio runtime)

---

## Data Structures

### 1. SearchResult (Core Data Structure)

**Location**: `Aether/core/src/search/result.rs` (new file)

```rust
use serde::{Deserialize, Serialize};

/// Search result entry returned by all providers
///
/// This struct provides a unified interface for search results from
/// different providers (Google, Bing, Tavily, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Result title
    pub title: String,

    /// Source URL
    pub url: String,

    /// Snippet/summary of the content
    pub snippet: String,

    /// Publication date (Unix timestamp)
    /// Optional because not all providers return this
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<i64>,

    /// Relevance score (0.0 - 1.0)
    /// Tavily provides this natively; others may compute it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,

    /// Source type (article, video, forum, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,

    /// Full content (only for Tavily deep search)
    /// WARNING: Can be very large, use sparingly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,

    /// Provider that returned this result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchResult {
    /// Create a basic search result (for testing/mocking)
    pub fn new(title: String, url: String, snippet: String) -> Self {
        Self {
            title,
            url,
            snippet,
            published_date: None,
            relevance_score: None,
            source_type: None,
            full_content: None,
            provider: None,
        }
    }

    /// Calculate content length (snippet + full_content)
    pub fn content_length(&self) -> usize {
        self.snippet.len() + self.full_content.as_ref().map(|c| c.len()).unwrap_or(0)
    }

    /// Check if result has full content
    pub fn has_full_content(&self) -> bool {
        self.full_content.is_some()
    }
}
```

**Design Rationale**:
- **Flat structure**: Easy to serialize for UniFFI (no nested enums)
- **Optional fields**: Accommodates different provider capabilities
- **Extensible**: New fields can be added without breaking existing code

### 2. SearchOptions (Configuration)

**Location**: `Aether/core/src/search/options.rs` (new file)

```rust
use serde::{Deserialize, Serialize};

/// Search options passed to providers
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Language code (ISO 639-1: "en", "zh", "ja", etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Region code (ISO 3166-1 alpha-2: "US", "CN", "JP", etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// Date range filter ("day", "week", "month", "year")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<String>,

    /// Enable safe search (adult content filtering)
    #[serde(default = "default_safe_search")]
    pub safe_search: bool,

    /// Maximum number of results (default: 5)
    #[serde(default = "default_max_results")]
    pub max_results: usize,

    /// Timeout in seconds (default: 10)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Include full page content (Tavily only)
    /// WARNING: Significantly increases latency and token usage
    #[serde(default)]
    pub include_full_content: bool,
}

fn default_safe_search() -> bool {
    true
}

fn default_max_results() -> usize {
    5
}

fn default_timeout() -> u64 {
    10
}

impl SearchOptions {
    /// Create default options
    pub fn default_with_timeout(timeout_seconds: u64) -> Self {
        Self {
            timeout_seconds,
            ..Default::default()
        }
    }
}
```

---

## Provider Abstraction Layer

### SearchProvider Trait

**Location**: `Aether/core/src/search/provider.rs` (new file)

```rust
use async_trait::async_trait;
use crate::error::Result;
use crate::search::{SearchResult, SearchOptions};

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
    /// * `Err(AetherError)` - Network error, API error, quota exceeded, etc.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// let provider = TavilyProvider::new("tvly-xxx".to_string())?;
    /// let options = SearchOptions::default();
    /// let results = provider.search("Rust async", &options).await?;
    /// ```
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>>;

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

/// Quota information for rate-limited providers
#[derive(Debug, Clone)]
pub struct QuotaInfo {
    /// Remaining searches in current period
    pub remaining: Option<u32>,

    /// Total quota limit
    pub limit: Option<u32>,

    /// Reset timestamp (Unix timestamp)
    pub reset_at: Option<i64>,
}

impl QuotaInfo {
    pub fn unlimited() -> Self {
        Self {
            remaining: None,
            limit: None,
            reset_at: None,
        }
    }
}
```

### SearchRegistry (Factory & Router)

**Location**: `Aether/core/src/search/registry.rs` (new file)

```rust
use std::collections::HashMap;
use std::sync::Arc;
use crate::config::SearchConfig;
use crate::error::{AetherError, Result};
use crate::search::{SearchProvider, SearchResult, SearchOptions};

/// Registry for managing multiple search providers
///
/// Maintains a pool of configured providers and routes search requests
/// to the appropriate backend based on configuration.
pub struct SearchRegistry {
    providers: HashMap<String, Arc<dyn SearchProvider>>,
    default_provider: String,
    fallback_providers: Vec<String>,
}

impl SearchRegistry {
    /// Create registry from configuration
    pub fn from_config(config: &SearchConfig) -> Result<Self> {
        let mut providers = HashMap::new();

        // Initialize configured providers
        for (name, backend_config) in &config.backends {
            let provider = create_provider(name, backend_config)?;
            if provider.is_available() {
                providers.insert(name.clone(), provider);
            } else {
                log::warn!("Search provider '{}' is not available (check API key)", name);
            }
        }

        if providers.is_empty() {
            return Err(AetherError::invalid_config(
                "No search providers configured or available"
            ));
        }

        Ok(Self {
            providers,
            default_provider: config.default_provider.clone(),
            fallback_providers: config.fallback_providers.clone().unwrap_or_default(),
        })
    }

    /// Execute search with fallback logic
    ///
    /// Tries default provider first, then falls back to alternatives if it fails
    pub async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        // Try default provider
        if let Some(provider) = self.providers.get(&self.default_provider) {
            match provider.search(query, options).await {
                Ok(results) => return Ok(results),
                Err(e) => {
                    log::warn!(
                        "Search failed with provider '{}': {}",
                        self.default_provider,
                        e
                    );
                }
            }
        }

        // Try fallback providers
        for provider_name in &self.fallback_providers {
            if let Some(provider) = self.providers.get(provider_name) {
                match provider.search(query, options).await {
                    Ok(results) => {
                        log::info!("Search succeeded with fallback provider '{}'", provider_name);
                        return Ok(results);
                    }
                    Err(e) => {
                        log::warn!("Fallback provider '{}' failed: {}", provider_name, e);
                    }
                }
            }
        }

        Err(AetherError::provider_error(format!(
            "All search providers failed for query: {}",
            query
        )))
    }
}

/// Factory function to create provider from config
fn create_provider(
    name: &str,
    config: &SearchBackendConfig,
) -> Result<Arc<dyn SearchProvider>> {
    match config.provider_type.as_str() {
        "tavily" => Ok(Arc::new(TavilyProvider::new(config.api_key.clone())?)),
        "searxng" => Ok(Arc::new(SearxngProvider::new(config.base_url.clone())?)),
        "brave" => Ok(Arc::new(BraveProvider::new(config.api_key.clone())?)),
        "google" => Ok(Arc::new(GoogleProvider::new(
            config.api_key.clone(),
            config.engine_id.clone()?,
        )?)),
        "bing" => Ok(Arc::new(BingProvider::new(config.api_key.clone())?)),
        "exa" => Ok(Arc::new(ExaProvider::new(config.api_key.clone())?)),
        _ => Err(AetherError::invalid_config(format!(
            "Unknown search provider type: {}",
            config.provider_type
        ))),
    }
}
```

---

## Provider Implementations

### Example: TavilyProvider (AI-Optimized)

**Location**: `Aether/core/src/search/providers/tavily.rs` (new file)

```rust
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::error::{AetherError, Result};
use crate::search::{SearchProvider, SearchResult, SearchOptions};

pub struct TavilyProvider {
    api_key: String,
    client: Client,
}

#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    search_depth: String,
    include_answer: bool,
    max_results: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_raw_content: Option<bool>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default)]
    raw_content: Option<String>,
}

impl TavilyProvider {
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(AetherError::invalid_config("Tavily API key is required"));
        }

        Ok(Self {
            api_key,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| AetherError::network_error(e.to_string()))?,
        })
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let request_body = TavilyRequest {
            api_key: self.api_key.clone(),
            query: query.to_string(),
            search_depth: if options.include_full_content {
                "advanced".to_string()
            } else {
                "basic".to_string()
            },
            include_answer: false,
            max_results: options.max_results,
            include_raw_content: if options.include_full_content {
                Some(true)
            } else {
                None
            },
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AetherError::network_error(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AetherError::provider_error(format!(
                "Tavily API error: {}",
                response.status()
            )));
        }

        let tavily_response: TavilyResponse = response
            .json()
            .await
            .map_err(|e| AetherError::provider_error(format!("Failed to parse Tavily response: {}", e)))?;

        // Convert to unified format
        let results = tavily_response
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                published_date: None,
                relevance_score: r.score,
                source_type: None,
                full_content: r.raw_content,
                provider: Some("tavily".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "tavily"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}
```

**Key Features**:
- Native relevance scoring
- Optional deep search with full content
- Clean, AI-optimized results
- Async execution with timeout

---

## Configuration Schema

### Search Configuration in `config.toml`

```toml
# ~/.aether/config.toml

[search]
enabled = true
default_provider = "tavily"
fallback_providers = ["searxng", "brave"]
max_results = 5
timeout_seconds = 10

# Provider-specific configurations
[search.backends.tavily]
provider_type = "tavily"
api_key = "tvly-xxx"

[search.backends.searxng]
provider_type = "searxng"
base_url = "http://localhost:8080"

[search.backends.brave]
provider_type = "brave"
api_key = "BSA_xxx"

[search.backends.google]
provider_type = "google"
api_key = "AIza_xxx"
engine_id = "cx_xxx"  # Custom Search Engine ID

[search.backends.bing]
provider_type = "bing"
api_key = "ocp-apim-xxx"

[search.backends.exa]
provider_type = "exa"
api_key = "exa_xxx"

# Routing rule with search capability
[[rules]]
regex = "^/search"
provider = "openai"
capabilities = ["search"]  # Enable search for this rule
system_prompt = "Summarize search results and answer the user's question."
```

### SearchConfig Struct

**Location**: `Aether/core/src/config/mod.rs` (extend existing)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub enabled: bool,

    pub default_provider: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,

    #[serde(default = "default_max_results")]
    pub max_results: usize,

    #[serde(default = "default_search_timeout")]
    pub timeout_seconds: u64,

    pub backends: HashMap<String, SearchBackendConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendConfig {
    pub provider_type: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,  // For Google CSE
}

fn default_max_results() -> usize {
    5
}

fn default_search_timeout() -> u64 {
    10
}
```

---

## Integration Flow

### End-to-End Request Flow

```
User Input: "/search 今日 AI 新闻"
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 1. Router.make_decision()               │
│    - Matches regex: "^/search"          │
│    - Sets Intent::BuiltinSearch         │
│    - Sets capabilities: [Search]        │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 2. PayloadBuilder.build()               │
│    - Creates AgentPayload               │
│    - Sets user_query: "今日 AI 新闻"    │
│    - Sets capabilities: [Search]        │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 3. CapabilityExecutor.execute_all()     │
│    - Iterates through capabilities      │
│    - Calls execute_search()             │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 4. execute_search()                     │
│    - Extracts query from payload        │
│    - Loads SearchOptions from config    │
│    - Calls SearchRegistry.search()      │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 5. SearchRegistry.search()              │
│    - Routes to Tavily provider          │
│    - Handles errors + fallback          │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 6. TavilyProvider.search()              │
│    - HTTP POST to Tavily API            │
│    - Parses JSON response               │
│    - Converts to SearchResult[]         │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 7. execute_search() (continued)         │
│    - Fills payload.context.search_results│
│    - Returns updated payload            │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 8. PromptAssembler.assemble()           │
│    - Reads payload.context.search_results│
│    - Formats as Markdown:               │
│      ## Search Results                  │
│      1. [Title](URL)                    │
│         Snippet...                      │
└─────────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│ 9. AI Provider (OpenAI/Claude)          │
│    - Receives full prompt with context  │
│    - Generates response                 │
└─────────────────────────────────────────┘
```

---

## Error Handling Strategy

### Error Categories

1. **Configuration Errors** (Fail Fast)
   - Missing API key
   - Invalid provider type
   - Malformed config file
   - **Action**: Return `AetherError::InvalidConfig`, log error, disable search

2. **Network Errors** (Retry + Fallback)
   - Connection timeout
   - DNS resolution failure
   - SSL/TLS errors
   - **Action**: Retry with exponential backoff, then fallback to alternative provider

3. **API Errors** (Graceful Degradation)
   - 401 Unauthorized (invalid API key)
   - 429 Too Many Requests (quota exceeded)
   - 500 Internal Server Error (provider downtime)
   - **Action**: Log error, attempt fallback provider, return empty results if all fail

4. **Parse Errors** (Log + Continue)
   - Unexpected JSON structure
   - Missing required fields
   - **Action**: Log warning, skip malformed results, return partial results

### Error Handling Code Example

```rust
// In CapabilityExecutor::execute_search()
pub fn execute_search(&self, payload: &mut AgentPayload) -> Result<()> {
    // Extract query safely
    let query = match Self::extract_search_query(&payload.user_query) {
        Some(q) => q,
        None => {
            log::warn!("Failed to extract search query from: {}", payload.user_query);
            return Ok(()); // Non-fatal: continue without search
        }
    };

    // Execute search with timeout
    let search_future = self.search_registry.search(&query, &self.search_options);
    let search_result = tokio::time::timeout(
        std::time::Duration::from_secs(self.search_options.timeout_seconds),
        search_future,
    ).await;

    match search_result {
        Ok(Ok(results)) => {
            // Success path
            payload.context.search_results = Some(results);
            log::info!("Search completed: {} results", results.len());
        }
        Ok(Err(e)) => {
            // Search failed but gracefully
            log::error!("Search failed: {}", e);
            payload.context.search_results = Some(vec![]);
            // Continue processing without search results
        }
        Err(_) => {
            // Timeout
            log::error!("Search timeout after {}s", self.search_options.timeout_seconds);
            payload.context.search_results = Some(vec![]);
        }
    }

    Ok(())
}
```

---

## Security & Privacy

### 1. PII Scrubbing

**Implementation**: `Aether/core/src/privacy/scrubber.rs` (existing module)

```rust
pub fn scrub_search_query(query: &str) -> String {
    let mut scrubbed = query.to_string();

    // Remove email addresses
    scrubbed = REGEX_EMAIL.replace_all(&scrubbed, "[EMAIL_REDACTED]").to_string();

    // Remove phone numbers
    scrubbed = REGEX_PHONE.replace_all(&scrubbed, "[PHONE_REDACTED]").to_string();

    // Remove credit card numbers
    scrubbed = REGEX_CREDIT_CARD.replace_all(&scrubbed, "[CC_REDACTED]").to_string();

    scrubbed
}
```

### 2. HTTPS-Only Communication

All providers must use HTTPS endpoints:
- ✅ Tavily: `https://api.tavily.com`
- ✅ Brave: `https://api.search.brave.com`
- ✅ Google: `https://www.googleapis.com`
- ✅ Bing: `https://api.bing.microsoft.com`
- ⚠️ SearXNG: User-configured (may be HTTP if self-hosted locally)

### 3. API Key Protection

- Store API keys in `config.toml` (user's home directory)
- Never log API keys (redact in logs)
- Use macOS Keychain integration (future enhancement)

---

## Performance Considerations

### 1. Timeout Management

- **Default**: 10 seconds per search
- **Configurable**: User can adjust in `config.toml`
- **Implementation**: `tokio::time::timeout()` wrapper

### 2. Concurrency

- **Multiple Searches**: Not supported in MVP (sequential execution only)
- **Future Enhancement**: Parallel queries to multiple providers with result merging

### 3. Result Caching

- **MVP**: No caching (every search hits external API)
- **Future Enhancement**: TTL-based cache with `lru` crate

---

## Testing Approach

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult::new(
            "Test Title".to_string(),
            "https://example.com".to_string(),
            "Test snippet".to_string(),
        );

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result, deserialized);
    }

    #[tokio::test]
    async fn test_mock_provider() {
        let provider = MockSearchProvider::new();
        let options = SearchOptions::default();
        let results = provider.search("test query", &options).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Mock Result");
    }
}
```

### Integration Tests

```rust
#[cfg(test)]
mod integration_tests {
    // Requires real API keys in environment variables

    #[tokio::test]
    #[ignore] // Skip in CI unless API keys are available
    async fn test_tavily_real_search() {
        let api_key = std::env::var("TAVILY_API_KEY").unwrap();
        let provider = TavilyProvider::new(api_key).unwrap();
        let options = SearchOptions::default();

        let results = provider.search("Rust programming language", &options).await.unwrap();

        assert!(!results.is_empty());
        assert!(results[0].url.starts_with("http"));
    }
}
```

---

## Future Enhancements

1. **Result Ranking Algorithm**: ML-based reranking for better relevance
2. **Multi-Provider Aggregation**: Query multiple providers in parallel, merge results
3. **Result Caching**: LRU cache with TTL to reduce API costs
4. **Cost Tracking**: Monitor API usage and costs
5. **Citation Formatting**: Inline citations in AI responses
6. **Web Scraping**: Fetch full page content for context (with rate limiting)
7. **Semantic Search**: Integration with vector embeddings for better matching

---

## Design Trade-offs

| Decision | Alternative | Rationale |
|----------|-------------|-----------|
| **Trait-based abstraction** | Direct provider calls | Enables runtime provider switching, easier testing |
| **No result caching in MVP** | Implement cache immediately | Complexity vs value; caching adds TTL management overhead |
| **Sequential fallback** | Parallel multi-provider query | Simpler implementation, lower API costs |
| **Markdown context format** | JSON or XML | Consistent with existing Memory capability, LLM-friendly |
| **No UI changes** | Add search UI panel | Focus on backend stability first |

---

## References

- **Architecture Doc**: `/docs/architecture/07_SEARCH_INTERFACE_RESERVATION.md`
- **Code Examples**: `/docs/architecture/search_code_example.md`
- **Tavily API**: https://docs.tavily.com
- **SearXNG Docs**: https://docs.searxng.org
- **Brave Search API**: https://brave.com/search/api/

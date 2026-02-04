# Spec Delta: search-capability

**Status**: Draft
**Capability**: search-capability
**Change**: add-search-capability-integration

---

## ADDED Requirements

### Requirement: SearchResult Data Structure

The system MUST define a `SearchResult` struct to represent individual search results returned by all providers with unified format.

#### Scenario: Creating a search result

```rust
use aethecore::search::SearchResult;

let result = SearchResult::new(
    "Rust Programming Language".to_string(),
    "https://www.rust-lang.org".to_string(),
    "A language empowering everyone to build reliable and efficient software.".to_string(),
);

assert_eq!(result.title, "Rust Programming Language");
assert_eq!(result.url, "https://www.rust-lang.org");
assert_eq!(result.snippet, "A language empowering everyone to build reliable and efficient software.");
assert!(result.published_date.is_none()); // Optional field
assert!(result.provider.is_none()); // Optional field
```

**Validation**: Struct can be created with minimal required fields.

#### Scenario: Serializing search result to JSON

```rust
use aethecore::search::SearchResult;
use serde_json;

let result = SearchResult {
    title: "Test".to_string(),
    url: "https://test.com".to_string(),
    snippet: "Test snippet".to_string(),
    published_date: Some(1704067200), // 2024-01-01
    relevance_score: Some(0.95),
    source_type: Some("article".to_string()),
    full_content: None,
    provider: Some("tavily".to_string()),
};

let json = serde_json::to_string(&result).unwrap();
assert!(json.contains("\"title\":\"Test\""));
assert!(json.contains("\"relevance_score\":0.95"));

// Deserialize back
let parsed: SearchResult = serde_json::from_str(&json).unwrap();
assert_eq!(parsed.title, "Test");
```

**Validation**: Struct correctly serializes/deserializes with optional fields.

---

### Requirement: SearchProvider Trait

The system MUST define a `SearchProvider` trait that all search backend implementations (Tavily, Google, Bing, etc.) implement for unified API.

#### Scenario: Implementing SearchProvider trait

```rust
use aethecore::search::{SearchProvider, SearchResult, SearchOptions};
use aethecore::error::Result;
use async_trait::async_trait;

struct MockSearchProvider;

#[async_trait]
impl SearchProvider for MockSearchProvider {
    async fn search(
        &self,
        query: &str,
        _options: &SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        Ok(vec![SearchResult::new(
            "Mock Title".to_string(),
            "https://mock.com".to_string(),
            format!("Mock result for query: {}", query),
        )])
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn is_available(&self) -> bool {
        true
    }
}

// Usage
#[tokio::test]
async fn test_mock_provider() {
    let provider = MockSearchProvider;
    let options = SearchOptions::default();
    let results = provider.search("test", &options).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Mock Title");
    assert_eq!(provider.name(), "mock");
    assert!(provider.is_available());
}
```

**Validation**: Trait can be implemented and provides consistent interface.

---

### Requirement: SearchOptions Configuration

The system MUST define `SearchOptions` struct to configure search behavior (language, region, filters, timeout).

#### Scenario: Creating search options with defaults

```rust
use aethecore::search::SearchOptions;

let options = SearchOptions::default();

assert_eq!(options.max_results, 5);
assert_eq!(options.timeout_seconds, 10);
assert!(options.safe_search);
assert!(!options.include_full_content);
assert!(options.language.is_none());
```

**Validation**: Default options are sensible and safe.

#### Scenario: Customizing search options

```rust
use aethecore::search::SearchOptions;

let options = SearchOptions {
    language: Some("zh-CN".to_string()),
    region: Some("CN".to_string()),
    date_range: Some("week".to_string()),
    safe_search: true,
    max_results: 10,
    timeout_seconds: 15,
    include_full_content: true,
};

assert_eq!(options.language.unwrap(), "zh-CN");
assert_eq!(options.max_results, 10);
assert!(options.include_full_content);
```

**Validation**: Options can be fully customized.

---

### Requirement: Tavily Provider Implementation

The system MUST implement `TavilyProvider` for Tavily AI search API with AI-optimized results.

#### Scenario: Creating Tavily provider

```rust
use aethecore::search::providers::TavilyProvider;

let provider = TavilyProvider::new("tvly-test-key".to_string()).unwrap();

assert_eq!(provider.name(), "tavily");
assert!(provider.is_available());
```

**Validation**: Provider can be created with valid API key.

#### Scenario: Tavily provider rejects empty API key

```rust
use aethecore::search::providers::TavilyProvider;

let result = TavilyProvider::new("".to_string());

assert!(result.is_err());
assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
```

**Validation**: Provider validates API key requirement.

#### Scenario: Executing Tavily search (integration test)

```rust
use aethecore::search::{providers::TavilyProvider, SearchOptions};

#[tokio::test]
#[ignore] // Requires real API key
async fn test_tavily_search() {
    let api_key = std::env::var("TAVILY_API_KEY").unwrap();
    let provider = TavilyProvider::new(api_key).unwrap();
    let options = SearchOptions::default();

    let results = provider.search("Rust programming language", &options).await.unwrap();

    assert!(!results.is_empty());
    assert!(results[0].url.starts_with("http"));
    assert!(!results[0].snippet.is_empty());
    assert_eq!(results[0].provider.as_ref().unwrap(), "tavily");
}
```

**Validation**: Real API calls return valid results.

---

### Requirement: SearXNG Provider Implementation

The system MUST implement `SearxngProvider` for self-hosted SearXNG instances with privacy-first approach.

#### Scenario: Creating SearXNG provider

```rust
use aethecore::search::providers::SearxngProvider;

let provider = SearxngProvider::new("http://localhost:8080".to_string()).unwrap();

assert_eq!(provider.name(), "searxng");
assert!(provider.is_available());
```

**Validation**: Provider can be created with base URL.

#### Scenario: SearXNG provider rejects empty URL

```rust
use aethecore::search::providers::SearxngProvider;

let result = SearxngProvider::new("".to_string());

assert!(result.is_err());
```

**Validation**: Provider validates base URL requirement.

---

### Requirement: Brave Provider Implementation

The system MUST implement `BraveProvider` for Brave Search API with privacy and quality balance.

#### Scenario: Creating Brave provider

```rust
use aethecore::search::providers::BraveProvider;

let provider = BraveProvider::new("BSA_test_key".to_string()).unwrap();

assert_eq!(provider.name(), "brave");
assert!(provider.is_available());
```

**Validation**: Provider can be created with valid API key.

---

### Requirement: Google CSE Provider Implementation

The system MUST implement `GoogleProvider` for Google Custom Search Engine with comprehensive coverage.

#### Scenario: Creating Google provider

```rust
use aethecore::search::providers::GoogleProvider;

let provider = GoogleProvider::new(
    "AIza_test_key".to_string(),
    "cx_test_engine".to_string(),
).unwrap();

assert_eq!(provider.name(), "google");
assert!(provider.is_available());
```

**Validation**: Provider requires both API key and engine ID.

---

### Requirement: Bing Provider Implementation

The system MUST implement `BingProvider` for Bing Web Search API with cost-effectiveness.

#### Scenario: Creating Bing provider

```rust
use aethecore::search::providers::BingProvider;

let provider = BingProvider::new("ocp-apim-test-key".to_string()).unwrap();

assert_eq!(provider.name(), "bing");
assert!(provider.is_available());
```

**Validation**: Provider can be created with subscription key.

---

### Requirement: Exa.ai Provider Implementation

The system MUST implement `ExaProvider` for Exa.ai semantic search API.

#### Scenario: Creating Exa provider

```rust
use aethecore::search::providers::ExaProvider;

let provider = ExaProvider::new("exa_test_key".to_string()).unwrap();

assert_eq!(provider.name(), "exa");
assert!(provider.is_available());
```

**Validation**: Provider can be created with valid API key.

---

### Requirement: SearchRegistry Factory

The system MUST implement `SearchRegistry` to manage multiple providers and route searches with fallback logic.

#### Scenario: Creating registry from config

```rust
use aethecore::search::SearchRegistry;
use aethecore::config::SearchConfig;
use std::collections::HashMap;

let mut backends = HashMap::new();
backends.insert("tavily".to_string(), SearchBackendConfig {
    provider_type: "tavily".to_string(),
    api_key: Some("tvly-key".to_string()),
    base_url: None,
    engine_id: None,
});

let config = SearchConfig {
    enabled: true,
    default_provider: "tavily".to_string(),
    fallback_providers: Some(vec!["searxng".to_string()]),
    max_results: 5,
    timeout_seconds: 10,
    backends,
};

let registry = SearchRegistry::from_config(&config).unwrap();
assert!(registry.get_provider("tavily").is_some());
```

**Validation**: Registry correctly initializes configured providers.

#### Scenario: Fallback to secondary provider on primary failure

```rust
use aethecore::search::{SearchRegistry, SearchOptions};

#[tokio::test]
async fn test_fallback_logic() {
    // Configure with primary (unavailable) and fallback (available)
    let registry = create_test_registry_with_fallback();
    let options = SearchOptions::default();

    // Primary provider will fail, should fallback
    let results = registry.search("test query", &options).await.unwrap();

    assert!(!results.is_empty());
    // Results should come from fallback provider
}
```

**Validation**: Registry automatically falls back on provider failure.

---

### Requirement: CapabilityExecutor Integration

The system MUST implement `CapabilityExecutor::execute_search()` to execute search capability and fill `AgentContext.search_results`.

#### Scenario: Executing search capability

```rust
use aethecore::capability::CapabilityExecutor;
use aethecore::payload::{AgentPayload, Capability};

#[tokio::test]
async fn test_execute_search() {
    let executor = CapabilityExecutor::new(/* with search registry */);

    let mut payload = AgentPayload::builder()
        .user_query("/search Rust async".to_string())
        .capabilities(vec![Capability::Search])
        .build()
        .unwrap();

    executor.execute_all(&mut payload).await.unwrap();

    assert!(payload.context.search_results.is_some());
    let results = payload.context.search_results.unwrap();
    assert!(!results.is_empty());
}
```

**Validation**: Search capability executes and fills context.

#### Scenario: Extracting search query from user input

```rust
use aethecore::capability::CapabilityExecutor;

#[test]
fn test_extract_search_query() {
    assert_eq!(
        CapabilityExecutor::extract_search_query("/search Rust async"),
        Some("Rust async".to_string())
    );

    assert_eq!(
        CapabilityExecutor::extract_search_query("/search   "),
        None // Empty query
    );

    assert_eq!(
        CapabilityExecutor::extract_search_query("not a search command"),
        None
    );
}
```

**Validation**: Query extraction handles edge cases.

---

### Requirement: Search Configuration Schema

The system MUST extend `Config` struct with `SearchConfig` for search feature configuration.

#### Scenario: Parsing search config from TOML

```toml
[search]
enabled = true
default_provider = "tavily"
fallback_providers = ["searxng", "brave"]
max_results = 5
timeout_seconds = 10

[search.backends.tavily]
provider_type = "tavily"
api_key = "tvly-xxx"
```

```rust
use aethecore::config::Config;

let config = Config::load_from_file("config.toml").unwrap();

assert!(config.search.is_some());
let search_config = config.search.unwrap();
assert!(search_config.enabled);
assert_eq!(search_config.default_provider, "tavily");
assert_eq!(search_config.max_results, 5);
assert!(search_config.backends.contains_key("tavily"));
```

**Validation**: Config correctly parses search section.

---

### Requirement: PromptAssembler Search Context Formatting

The system MUST format search results in Markdown when assembling prompts.

#### Scenario: Formatting search results in prompt

```rust
use aethecore::payload::{PromptAssembler, AgentPayload, ContextFormat};
use aethecore::search::SearchResult;

let mut payload = AgentPayload::builder()
    .user_query("What is Rust?".to_string())
    .context_format(ContextFormat::Markdown)
    .build()
    .unwrap();

payload.context.search_results = Some(vec![
    SearchResult::new(
        "Rust Programming Language".to_string(),
        "https://www.rust-lang.org".to_string(),
        "A language empowering everyone...".to_string(),
    ),
]);

let assembler = PromptAssembler::new();
let prompt = assembler.assemble(&payload).unwrap();

assert!(prompt.contains("## Search Results"));
assert!(prompt.contains("1. [Rust Programming Language](https://www.rust-lang.org)"));
assert!(prompt.contains("A language empowering everyone..."));
```

**Validation**: Search results are formatted as Markdown citations.

---

### Requirement: Error Handling and Graceful Degradation

The system MUST handle search failures gracefully without crashing the application.

#### Scenario: Search timeout does not crash system

```rust
use aethecore::capability::CapabilityExecutor;
use aethecore::payload::{AgentPayload, Capability};

#[tokio::test]
async fn test_search_timeout() {
    let executor = CapabilityExecutor::new(/* with slow provider */);

    let mut payload = AgentPayload::builder()
        .user_query("/search test".to_string())
        .capabilities(vec![Capability::Search])
        .build()
        .unwrap();

    // Should not panic
    let result = executor.execute_all(&mut payload).await;

    assert!(result.is_ok());
    // Results may be empty, but system continues
    assert!(payload.context.search_results.is_some());
}
```

**Validation**: Timeout handled gracefully.

#### Scenario: Invalid API key returns error but continues

```rust
use aethecore::search::providers::TavilyProvider;
use aethecore::search::SearchOptions;

#[tokio::test]
async fn test_invalid_api_key() {
    let provider = TavilyProvider::new("invalid-key".to_string()).unwrap();
    let options = SearchOptions::default();

    let result = provider.search("test", &options).await;

    assert!(result.is_err());
    assert!(matches!(result, Err(AlephError::ProviderError { .. })));
}
```

**Validation**: Authentication errors are properly typed.

---

### Requirement: Privacy and PII Scrubbing

The system MUST scrub PII from search queries before sending to external APIs.

#### Scenario: Scrubbing email from query

```rust
use aethecore::privacy::scrub_search_query;

let query = "Contact john.doe@example.com for help";
let scrubbed = scrub_search_query(query);

assert_eq!(scrubbed, "Contact [EMAIL_REDACTED] for help");
```

**Validation**: Email addresses are redacted.

#### Scenario: Scrubbing phone number from query

```rust
use aethecore::privacy::scrub_search_query;

let query = "Call +1-555-123-4567 for support";
let scrubbed = scrub_search_query(query);

assert_eq!(scrubbed, "Call [PHONE_REDACTED] for support");
```

**Validation**: Phone numbers are redacted.

---

## MODIFIED Requirements

### Requirement: AgentContext Must Include search_results Field

The existing `AgentContext` struct MUST have `search_results` field properly defined (currently reserved but type incomplete).

#### Scenario: AgentContext includes search results

```rust
use aethecore::payload::{AgentContext, ContextFormat};
use aethecore::search::SearchResult;

let mut context = AgentContext::new(ContextFormat::Markdown);
context.search_results = Some(vec![
    SearchResult::new(
        "Title".to_string(),
        "https://example.com".to_string(),
        "Snippet".to_string(),
    ),
]);

assert!(context.search_results.is_some());
assert_eq!(context.search_results.unwrap().len(), 1);
```

**Validation**: Field is properly typed and usable.

---

### Requirement: RoutingRuleConfig Supports Search Capability

The existing `RoutingRuleConfig` MUST parse `capabilities = ["search"]` from config.

#### Scenario: Parsing rule with search capability

```toml
[[rules]]
regex = "^/search"
provider = "openai"
capabilities = ["search"]
```

```rust
use aethecore::config::Config;

let config = Config::load_from_str(toml_str).unwrap();
let rule = &config.rules[0];

assert_eq!(rule.regex, "^/search");
assert!(rule.capabilities.contains(&"search"));
```

**Validation**: Search capability is correctly parsed from config.

---

## Summary

This spec delta adds **18 new requirements** covering:

1. **Core Data Structures**: `SearchResult`, `SearchOptions`
2. **Abstraction Layer**: `SearchProvider` trait, `SearchRegistry`
3. **Provider Implementations**: Tavily, SearXNG, Brave, Google, Bing, Exa
4. **Integration**: `CapabilityExecutor::execute_search()`, prompt formatting
5. **Configuration**: `SearchConfig` schema and parsing
6. **Safety**: Error handling, privacy, graceful degradation

All requirements include concrete scenarios demonstrating expected behavior.

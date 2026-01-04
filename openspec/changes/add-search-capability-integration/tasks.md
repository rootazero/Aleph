# Implementation Tasks: add-search-capability-integration

**Status**: Draft
**Total Tasks**: 42
**Estimated Duration**: 3-4 weeks

---

## Phase 1: Core Infrastructure (Week 1)

### Module Structure Setup

- [ ] Create `Aether/core/src/search/` module directory
- [ ] Create `Aether/core/src/search/mod.rs` with public API
- [ ] Create `Aether/core/src/search/result.rs` for `SearchResult` struct
- [ ] Create `Aether/core/src/search/options.rs` for `SearchOptions` struct
- [ ] Create `Aether/core/src/search/provider.rs` for `SearchProvider` trait
- [ ] Create `Aether/core/src/search/registry.rs` for `SearchRegistry`
- [ ] Create `Aether/core/src/search/providers/` directory for implementations
- [ ] Add `async-trait` dependency to `Cargo.toml`

**Validation**: `cargo build` succeeds, all modules importable.

---

### Data Structures Implementation

- [ ] Implement `SearchResult` struct with all fields
  - [ ] Add `new()` constructor
  - [ ] Add `content_length()` helper
  - [ ] Add `has_full_content()` helper
  - [ ] Add serde `Serialize` + `Deserialize` derives
- [ ] Implement `SearchOptions` struct
  - [ ] Add default value functions
  - [ ] Add `Default` impl
  - [ ] Add `default_with_timeout()` constructor
- [ ] Implement `QuotaInfo` struct
  - [ ] Add `unlimited()` constructor

**Validation**: Unit tests for serialization/deserialization pass.

---

### SearchProvider Trait Definition

- [ ] Define `SearchProvider` trait with methods:
  - [ ] `async fn search(&self, query, options) -> Result<Vec<SearchResult>>`
  - [ ] `fn name(&self) -> &str`
  - [ ] `fn is_available(&self) -> bool`
  - [ ] `async fn get_quota(&self) -> Result<QuotaInfo>` (default impl)
- [ ] Add `async_trait` macro to trait definition
- [ ] Create mock implementation for testing

**Validation**: Mock provider compiles and passes basic trait tests.

---

## Phase 2: Provider Implementations (Week 2-3)

### Tavily Provider

- [ ] Create `Aether/core/src/search/providers/tavily.rs`
- [ ] Implement `TavilyProvider` struct with `api_key` and `client` fields
- [ ] Implement private `TavilyRequest` and `TavilyResponse` structs
- [ ] Implement `SearchProvider` trait for `TavilyProvider`
- [ ] Add `new()` constructor with API key validation
- [ ] Implement HTTP POST to `https://api.tavily.com/search`
- [ ] Parse JSON response and convert to `SearchResult`
- [ ] Handle errors (network, auth, rate limit)
- [ ] Add unit tests with mock HTTP responses
- [ ] Add integration test with real API (mark as `#[ignore]`)

**Validation**: Integration test passes with valid API key.

---

### SearXNG Provider

- [ ] Create `Aether/core/src/search/providers/searxng.rs`
- [ ] Implement `SearxngProvider` struct with `base_url` and `client`
- [ ] Implement private `SearxngResponse` and `SearxngResult` structs
- [ ] Implement `SearchProvider` trait for `SearxngProvider`
- [ ] Add `new()` constructor with base URL validation
- [ ] Implement HTTP GET to `{base_url}/search?q={query}&format=json`
- [ ] Parse JSON response and convert to `SearchResult`
- [ ] Handle self-hosted instance availability checks
- [ ] Add unit tests
- [ ] Add integration test with public SearXNG instance (if available)

**Validation**: Integration test succeeds with public instance or localhost.

---

### Brave Provider

- [ ] Create `Aether/core/src/search/providers/brave.rs`
- [ ] Implement `BraveProvider` struct
- [ ] Implement private `BraveResponse` and `BraveResult` structs
- [ ] Implement `SearchProvider` trait
- [ ] Add `new()` constructor
- [ ] Implement HTTP GET to `https://api.search.brave.com/res/v1/web/search`
- [ ] Add `X-Subscription-Token` header
- [ ] Parse nested JSON structure (`web.results`)
- [ ] Convert `description` field to `snippet`
- [ ] Add tests

**Validation**: Tests pass with valid Brave API key.

---

### Google CSE Provider

- [ ] Create `Aether/core/src/search/providers/google.rs`
- [ ] Implement `GoogleProvider` struct with `api_key` and `engine_id`
- [ ] Implement private `GoogleResponse` and `GoogleItem` structs
- [ ] Implement `SearchProvider` trait
- [ ] Add `new()` constructor requiring both API key and engine ID
- [ ] Implement HTTP GET to `https://www.googleapis.com/customsearch/v1`
- [ ] Parse `items` array from response
- [ ] Convert `snippet` field to `SearchResult.snippet`
- [ ] Handle quota limits (100/day free tier)
- [ ] Add tests

**Validation**: Integration test passes with Google API credentials.

---

### Bing Provider

- [ ] Create `Aether/core/src/search/providers/bing.rs`
- [ ] Implement `BingProvider` struct
- [ ] Implement private `BingResponse` and `BingWebPage` structs
- [ ] Implement `SearchProvider` trait
- [ ] Add `new()` constructor
- [ ] Implement HTTP GET to `https://api.bing.microsoft.com/v7.0/search`
- [ ] Add `Ocp-Apim-Subscription-Key` header
- [ ] Parse nested `webPages.value` structure
- [ ] Extract `name` → `title`, `snippet` → `snippet`
- [ ] Add tests

**Validation**: Tests pass with Bing subscription key.

---

### Exa.ai Provider

- [ ] Create `Aether/core/src/search/providers/exa.rs`
- [ ] Implement `ExaProvider` struct
- [ ] Implement private `ExaRequest` and `ExaResponse` structs
- [ ] Implement `SearchProvider` trait
- [ ] Add `new()` constructor
- [ ] Implement HTTP POST to `https://api.exa.ai/search`
- [ ] Add `x-api-key` header
- [ ] Request `contents.text` in payload
- [ ] Parse `results` array and extract `text` field
- [ ] Add tests

**Validation**: Tests pass with Exa API key.

---

### Provider Registry

- [ ] Implement `SearchRegistry::from_config()` constructor
- [ ] Create factory function `create_provider(name, config)`
- [ ] Implement provider availability checking
- [ ] Implement primary + fallback search logic in `search()` method
- [ ] Add logging for provider selection and failures
- [ ] Handle case where all providers fail (return error)
- [ ] Add tests for fallback behavior

**Validation**: Registry correctly routes to fallback on primary failure.

---

## Phase 3: Configuration Integration (Week 3)

### Config Schema Extension

- [ ] Add `SearchConfig` struct to `Aether/core/src/config/mod.rs`
  - [ ] `enabled: bool`
  - [ ] `default_provider: String`
  - [ ] `fallback_providers: Option<Vec<String>>`
  - [ ] `max_results: usize`
  - [ ] `timeout_seconds: u64`
  - [ ] `backends: HashMap<String, SearchBackendConfig>`
- [ ] Add `SearchBackendConfig` struct
  - [ ] `provider_type: String`
  - [ ] `api_key: Option<String>`
  - [ ] `base_url: Option<String>`
  - [ ] `engine_id: Option<String>`
- [ ] Add `search: Option<SearchConfig>` field to root `Config` struct
- [ ] Add default value functions
- [ ] Add tests for config parsing

**Validation**: Example `config.toml` with search section parses correctly.

---

### Example Configuration

- [ ] Create `examples/config-with-search.toml`
- [ ] Document all search config options
- [ ] Provide examples for each provider type
- [ ] Include fallback configuration example

**Validation**: Example config loads without errors.

---

## Phase 4: Capability Executor Integration (Week 3-4)

### CapabilityExecutor Extension

- [ ] Add `SearchRegistry` field to `CapabilityExecutor` struct
- [ ] Add `search_options: SearchOptions` field
- [ ] Modify `CapabilityExecutor::new()` to accept optional `SearchRegistry`
- [ ] Implement `extract_search_query(input: &str) -> Option<String>` helper
  - [ ] Match `/search ` prefix
  - [ ] Trim whitespace
  - [ ] Return `None` if query is empty
- [ ] Implement `execute_search(&self, payload: &mut AgentPayload) -> Result<()>`
  - [ ] Extract query from `payload.user_query`
  - [ ] Call `SearchRegistry.search()`
  - [ ] Fill `payload.context.search_results`
  - [ ] Handle timeout with `tokio::time::timeout()`
  - [ ] Log errors but don't crash
- [ ] Add search capability to capability execution loop in `execute_all()`
- [ ] Add tests for search capability execution

**Validation**: End-to-end test passes: routing rule → capability execution → results in payload.

---

### AgentContext Modification

- [ ] Verify `AgentContext.search_results: Option<Vec<SearchResult>>` field exists
- [ ] If not, add it to struct definition
- [ ] Update `AgentContext::new()` to initialize field as `None`

**Validation**: `AgentContext` compiles with search results field.

---

## Phase 5: Prompt Assembly Integration (Week 4)

### PromptAssembler Extension

- [ ] Modify `PromptAssembler::assemble()` to handle search results
- [ ] Implement `format_search_results_markdown()` helper
  - [ ] Format as numbered list with Markdown links
  - [ ] Include snippet text
  - [ ] Add optional published date
- [ ] Add search results section to final prompt
- [ ] Add tests for prompt formatting

**Validation**: Assembled prompt includes formatted search results.

---

## Phase 6: Privacy & Security (Week 4)

### PII Scrubbing

- [ ] Add `scrub_search_query()` function to `Aether/core/src/privacy/scrubber.rs`
- [ ] Implement regex-based email redaction
- [ ] Implement regex-based phone number redaction
- [ ] Implement regex-based credit card redaction
- [ ] Call scrubber in `execute_search()` before external API calls
- [ ] Add tests for PII scrubbing

**Validation**: Queries with PII are properly redacted.

---

### HTTPS Enforcement

- [ ] Verify all provider endpoints use HTTPS
- [ ] Document exception for SearXNG (user-configured)
- [ ] Add warning if SearXNG uses HTTP

**Validation**: Security audit passes.

---

## Phase 7: Error Handling & Testing (Week 4)

### Error Handling

- [ ] Add search-specific error variants to `AetherError` (if needed)
- [ ] Implement graceful degradation on search timeout
- [ ] Implement graceful degradation on all providers failing
- [ ] Add retry logic with exponential backoff (optional)
- [ ] Log all errors with structured logging

**Validation**: No panics occur on network failures or invalid API keys.

---

### Unit Tests

- [ ] Test `SearchResult` serialization
- [ ] Test `SearchOptions` defaults
- [ ] Test `SearchProvider` mock implementation
- [ ] Test `SearchRegistry` fallback logic
- [ ] Test PII scrubbing
- [ ] Test query extraction from `/search` command

**Validation**: `cargo test` passes with 100% success rate.

---

### Integration Tests

- [ ] Create `tests/search_integration.rs`
- [ ] Test real Tavily API call (with `#[ignore]`)
- [ ] Test real SearXNG call (with `#[ignore]`)
- [ ] Test end-to-end flow: user input → routing → search → prompt

**Validation**: Integration tests pass with real API keys.

---

## Phase 8: Documentation & Examples (Week 4)

### Code Documentation

- [ ] Add rustdoc comments to all public structs/traits
- [ ] Add usage examples in doc comments
- [ ] Document error cases

**Validation**: `cargo doc --open` shows complete documentation.

---

### User Documentation

- [ ] Update `/docs/ARCHITECTURE.md` with search capability details
- [ ] Update `/CLAUDE.md` with search configuration instructions
- [ ] Create `/docs/SEARCH_GUIDE.md` with:
  - [ ] Provider comparison table
  - [ ] Setup instructions for each provider
  - [ ] Configuration examples
  - [ ] Troubleshooting guide

**Validation**: Documentation is clear and complete.

---

### Example Configurations

- [ ] Create `examples/tavily-config.toml`
- [ ] Create `examples/searxng-config.toml`
- [ ] Create `examples/multi-provider-config.toml`

**Validation**: All example configs are valid and loadable.

---

## Phase 9: Final Validation & Cleanup

### Code Quality

- [ ] Run `cargo fmt` on all new files
- [ ] Run `cargo clippy` and fix all warnings
- [ ] Remove debug print statements
- [ ] Ensure no `TODO` or `FIXME` comments remain

**Validation**: `cargo clippy -- -D warnings` passes.

---

### Performance Testing

- [ ] Measure search latency for each provider
- [ ] Verify timeout enforcement works correctly
- [ ] Check memory usage (no leaks)

**Validation**: Latency < 10s for all providers (under normal network conditions).

---

### Security Audit

- [ ] Verify no API keys are logged
- [ ] Verify PII scrubbing is applied before API calls
- [ ] Verify HTTPS is used for all cloud providers
- [ ] Check for SQL injection vulnerabilities (N/A for this change)

**Validation**: Security checklist approved.

---

### Pre-Merge Checklist

- [ ] All unit tests pass (`cargo test`)
- [ ] All integration tests pass (with API keys)
- [ ] Documentation is complete
- [ ] `cargo clippy` passes
- [ ] `cargo fmt` applied
- [ ] No breaking changes to existing code
- [ ] `openspec validate add-search-capability-integration --strict` passes

**Validation**: Ready for code review.

---

## Notes

- **API Keys**: Store in environment variables for testing, never commit to repo
- **Rate Limits**: Respect provider rate limits during testing
- **Dependencies**: Minimize new dependencies, reuse existing `reqwest`
- **Testing**: Prioritize unit tests over integration tests for CI/CD compatibility

---

## Success Criteria

All tasks completed when:
1. User can configure search in `config.toml`
2. `/search <query>` command returns results
3. Results are formatted in AI prompt context
4. Fallback providers work on primary failure
5. No crashes on network errors or invalid API keys
6. All tests pass
7. Documentation is complete

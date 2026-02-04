# Change Proposal: add-search-capability-integration

**Status**: Draft
**Created**: 2026-01-04
**Author**: AI Assistant
**Related Changes**: implement-structured-context-protocol

---

## Summary

Implement real-time web search capability for Aleph Agent, enabling AI to access up-to-date information beyond training data cutoff. This change integrates multiple third-party search providers (Google, Bing, Tavily, Exa.ai, SearXNG, Brave) through a unified abstraction layer.

**Business Value**:
- ✅ **Information Timeliness**: Access to latest news, real-time data, current events
- ✅ **Knowledge Expansion**: Access to specialized domain knowledge beyond AI training data
- ✅ **Fact Verification**: Cross-validate AI responses with current web sources
- ✅ **Multi-source Aggregation**: Comprehensive insights from multiple search backends

---

## Background

### Current State

The Structured Context Protocol implementation (change: `implement-structured-context-protocol`) has already **reserved the Search capability interface**:

- ✅ `Capability::Search` enum variant defined in `Aleph/core/src/payload/capability.rs`
- ✅ `Intent::BuiltinSearch` for `/search` command classification
- ✅ `AgentContext.search_results` field reserved (currently `Option<Vec<SearchResult>>`)
- ⚠️ `SearchResult` struct **not yet defined** (needs creation)
- ⚠️ `CapabilityExecutor::execute_search()` returns warning only (no implementation)

**Reference Documentation**: `/docs/architecture/07_SEARCH_INTERFACE_RESERVATION.md` provides comprehensive design guidelines.

### Problem Statement

Users currently cannot:
1. Ask about current events ("今日 AI 新闻")
2. Query real-time data ("比特币当前价格")
3. Research latest technical documentation ("Rust async 最佳实践 2026")
4. Access information published after AI training cutoff

### Why Multiple Providers?

Different use cases require different search backends:

| Provider | Best For | Cost | Privacy |
|----------|----------|------|---------|
| **Tavily** | AI agents (optimized results) | $0.005/search | Medium |
| **SearXNG** | Privacy-first, unlimited usage | Free (self-hosted) | High |
| **Brave** | Privacy + quality balance | $3/1000 | High |
| **Google CSE** | Comprehensive coverage | $5/1000 | Low |
| **Bing** | Cost-effective | $3/1000 | Low |
| **Exa.ai** | Semantic search | Variable | Medium |

**Design Philosophy**: Plugin architecture allows users to choose based on their priorities (cost, privacy, quality).

---

## Scope

### In Scope

1. **Core Search Infrastructure**
   - `SearchResult` struct definition
   - `SearchProvider` trait abstraction
   - `SearchOptions` configuration struct
   - Integration into `CapabilityExecutor`

2. **Provider Implementations**
   - Tavily AI (recommended default)
   - SearXNG (privacy-first)
   - Brave Search (balanced)
   - Google CSE (comprehensive)
   - Bing Search API (cost-effective)
   - Exa.ai (semantic search)

3. **Configuration System**
   - `SearchConfig` in `config.toml`
   - Per-provider API key management
   - Search behavior customization (max results, timeout, filters)

4. **Routing Integration**
   - `/search <query>` command detection
   - Automatic search capability activation based on routing rules
   - Search results formatting in prompt context

5. **Error Handling**
   - Network failures
   - API quota limits
   - Authentication errors
   - Fallback to alternative providers

### Out of Scope

- ❌ **UI/UX Changes**: No macOS client UI modifications (backend-only)
- ❌ **Cost Tracking**: No built-in usage analytics (future enhancement)
- ❌ **Result Caching**: No persistent search cache (future enhancement)
- ❌ **Custom Ranking**: No ML-based result reranking (use provider's native ranking)

### Constraints

1. **No Breaking Changes**: Must not modify existing `AgentPayload` structure
2. **Async-Only**: All search operations must be async (`tokio` runtime)
3. **Privacy-First**: PII scrubbing before sending queries to cloud APIs
4. **Timeout Enforcement**: All searches must respect configured timeout (default 10s)
5. **Graceful Degradation**: System continues working if search provider unavailable

---

## Dependencies

### Internal Dependencies

- ✅ `Capability::Search` enum (already defined)
- ✅ `Intent::BuiltinSearch` enum (already defined)
- ✅ `AgentContext.search_results` field (already reserved)
- 🆕 Requires new `SearchResult` struct
- 🆕 Requires new `search` module with provider implementations

### External Dependencies (Rust Crates)

```toml
[dependencies]
# Existing (no changes)
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1", features = ["full"] }

# New dependencies for search
async-trait = "0.1"  # For async trait methods
```

### API Requirements

Each provider requires API credentials (stored in `config.toml`):

| Provider | Credential Type | Free Tier | Documentation |
|----------|----------------|-----------|---------------|
| Tavily | API Key | 1000/month | https://tavily.com |
| SearXNG | Base URL | Unlimited | https://searx.github.io/searx/ |
| Brave | API Key | Free tier available | https://brave.com/search/api/ |
| Google CSE | API Key + Engine ID | 100/day | https://developers.google.com/custom-search |
| Bing | Subscription Key | 1000/month | https://www.microsoft.com/en-us/bing/apis/bing-web-search-api |
| Exa.ai | API Key | Variable | https://exa.ai |

---

## Success Criteria

### Functional Requirements

1. **Search Execution**
   - [ ] User types `/search 今日AI新闻`, receives latest results
   - [ ] Search results include title, URL, snippet
   - [ ] Results are injected into prompt context before AI generation

2. **Multi-Provider Support**
   - [ ] Can switch providers via `config.toml`
   - [ ] Fallback to secondary provider on primary failure
   - [ ] Each provider returns unified `SearchResult` format

3. **Configuration**
   - [ ] Search feature can be enabled/disabled globally
   - [ ] Per-rule capability activation (`capabilities = ["search"]`)
   - [ ] Provider-specific options (language, region, safe search)

4. **Error Handling**
   - [ ] Network timeout returns error (no crash)
   - [ ] Invalid API key shows clear error message
   - [ ] Quota exceeded triggers fallback provider

### Non-Functional Requirements

1. **Performance**: Search completes within 10 seconds (configurable timeout)
2. **Reliability**: 99% success rate on valid API keys
3. **Privacy**: No query logging, PII scrubbing before API calls
4. **Maintainability**: Each provider in separate module (`search/providers/*.rs`)

---

## Testing Strategy

### Unit Tests

- `SearchResult` struct serialization/deserialization
- `SearchProvider` trait mock implementation
- Config parsing for `SearchConfig`

### Integration Tests

- Real API calls to each provider (with test API keys)
- Fallback behavior when primary provider fails
- Search results formatting in `PromptAssembler`

### Manual Testing

- `/search <query>` command in macOS client
- Different providers with real API keys
- Error scenarios (invalid key, network offline)

---

## Migration Plan

### Phase 1: Core Infrastructure (Week 1)

1. Define `SearchResult` struct
2. Define `SearchProvider` trait
3. Implement `CapabilityExecutor::execute_search()` (no actual search)
4. Add `SearchConfig` to config schema

### Phase 2: Provider Implementation (Week 2-3)

1. Implement Tavily provider (recommended default)
2. Implement SearXNG provider (privacy-first)
3. Implement Brave provider
4. Implement Google/Bing providers
5. Implement Exa.ai provider

### Phase 3: Integration & Testing (Week 4)

1. Integrate with routing rules
2. Add `/search` command detection
3. Format search results in prompt context
4. Error handling and fallback logic
5. Documentation and examples

### Rollback Plan

If issues arise:
1. Set `search.enabled = false` in config (disables feature)
2. Remove `capabilities = ["search"]` from routing rules
3. System continues working with Memory-only capabilities

---

## Risks & Mitigation

### Risk 1: API Cost Overrun

**Impact**: High usage could exceed free tier quotas
**Mitigation**:
- Default to free SearXNG for testing
- Add config option for max searches per day
- Log warning when approaching quota limits

### Risk 2: Provider API Changes

**Impact**: Breaking changes in third-party APIs
**Mitigation**:
- Abstract behind `SearchProvider` trait
- Version-specific adapters
- Comprehensive error handling

### Risk 3: Privacy Concerns

**Impact**: User queries sent to third-party services
**Mitigation**:
- PII scrubbing before API calls
- Prefer privacy-focused providers (SearXNG, Brave)
- Clear documentation on data flow

### Risk 4: Network Latency

**Impact**: Slow search results degrade UX
**Mitigation**:
- Configurable timeout (default 10s)
- Async execution (non-blocking)
- Show loading state in Halo overlay

---

## Related Documentation

- Architecture: `/docs/architecture/07_SEARCH_INTERFACE_RESERVATION.md`
- Code Examples: `/docs/architecture/search_code_example.md`
- Structured Context Protocol: `/docs/ARCHITECTURE.md`
- Change Proposal: `implement-structured-context-protocol`

---

## Open Questions

1. **Result Ranking**: Should we implement our own ranking algorithm or trust provider's native ranking?
   → **Decision**: Trust provider ranking (out of scope for MVP)

2. **Result Caching**: Should we cache search results to reduce API calls?
   → **Decision**: No caching in MVP (future enhancement)

3. **Multi-Provider Aggregation**: Should we query multiple providers and merge results?
   → **Decision**: Single provider per query (MVP), future enhancement for aggregation

4. **Default Provider**: Which provider should be recommended default?
   → **Decision**: Tavily (AI-optimized, good free tier)

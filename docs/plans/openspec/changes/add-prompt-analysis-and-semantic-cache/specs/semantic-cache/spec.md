# Spec: semantic-cache

Semantic caching capability for reusing responses to semantically similar prompts.

## ADDED Requirements

### Requirement: Exact Match Lookup
The system MUST support fast exact-match cache lookups.

#### Scenario: Exact prompt match returns cached response
- Given a prompt "What is Rust?" was previously cached
- And the same exact prompt "What is Rust?" is submitted
- When the system performs cache lookup
- Then the cached response is returned
- And the hit type is Exact

#### Scenario: Exact match is case-sensitive
- Given "What is Rust?" is cached
- And "what is rust?" is submitted
- When the system performs exact match lookup
- Then no exact match is found

#### Scenario: Exact match latency
- Given a cache with 10,000 entries
- When an exact match lookup is performed
- Then the lookup completes within 1ms

### Requirement: Semantic Similarity Lookup
The system MUST support semantic similarity-based cache lookups.

#### Scenario: Similar prompt returns cached response
- Given "What is Rust programming language?" was cached
- And "Tell me about Rust" is submitted
- When the system performs semantic lookup
- Then the cached response is returned if similarity > threshold
- And the hit type is Semantic

#### Scenario: Dissimilar prompt misses cache
- Given "What is Rust?" was cached
- And "How do I cook pasta?" is submitted
- When the system performs semantic lookup
- Then no cache hit is returned

#### Scenario: Similarity threshold is configurable
- Given a similarity threshold of 0.85 is configured
- When a prompt with 0.80 similarity to a cached entry is submitted
- Then the cache lookup returns a miss

#### Scenario: Semantic lookup latency
- Given a cache with 10,000 entries
- When a semantic similarity lookup is performed (including embedding)
- Then the lookup completes within 60ms

### Requirement: Cache Storage
The system MUST store prompt-response pairs with metadata.

#### Scenario: Store new entry
- Given a prompt "Explain quicksort" and response from claude-sonnet
- When the system stores the entry
- Then the entry is retrievable by exact match
- And the entry is retrievable by semantic similarity

#### Scenario: Entry contains required metadata
- Given a stored cache entry
- Then the entry contains:
  - prompt_hash (String)
  - embedding (Vec<f32>)
  - response_content (String)
  - model_used (String)
  - created_at (timestamp)
  - tokens_used (u32)
  - latency_ms (u64)
  - cost_usd (f64)

#### Scenario: Duplicate prompt updates existing entry
- Given "What is Rust?" is already cached
- When the same prompt is stored again with a new response
- Then the existing entry is updated (not duplicated)

### Requirement: TTL Management
The system MUST support time-to-live expiration for cache entries.

#### Scenario: Entry with TTL expires
- Given an entry stored with TTL of 1 hour
- When 2 hours have passed
- Then the entry is not returned in lookups

#### Scenario: Default TTL applied
- Given a default TTL of 24 hours is configured
- When an entry is stored without explicit TTL
- Then the entry expires after 24 hours

#### Scenario: Maximum TTL enforced
- Given a maximum TTL of 7 days is configured
- When an entry is stored with TTL of 30 days
- Then the effective TTL is capped at 7 days

### Requirement: Eviction Policies
The system MUST evict entries when capacity is exceeded.

#### Scenario: Expired entries evicted first
- Given the cache is at capacity
- And some entries have expired
- When a new entry is stored
- Then expired entries are evicted first

#### Scenario: LRU eviction
- Given LRU eviction policy is configured
- And the cache is at capacity with no expired entries
- When a new entry is stored
- Then the least recently accessed entry is evicted

#### Scenario: LFU eviction
- Given LFU eviction policy is configured
- And the cache is at capacity
- When a new entry is stored
- Then the least frequently hit entry is evicted

#### Scenario: Hybrid eviction
- Given Hybrid eviction policy with age_weight=0.4 and hits_weight=0.6
- When eviction is needed
- Then entries are scored by combined age and hit count
- And lowest scoring entries are evicted

### Requirement: Cache Statistics
The system MUST provide cache performance statistics.

#### Scenario: Basic stats available
- Given cache operations have occurred
- When stats are requested
- Then the response includes:
  - total_entries (usize)
  - hit_count (u64)
  - miss_count (u64)
  - hit_rate (f64)

#### Scenario: Hit type breakdown
- Given both exact and semantic hits have occurred
- When stats are requested
- Then the response includes exact_hits and semantic_hits counts

#### Scenario: Memory usage tracked
- Given entries are stored in cache
- When stats are requested
- Then total_size_bytes reflects approximate memory usage

### Requirement: Cache Management
The system MUST support manual cache management operations.

#### Scenario: Invalidate specific entry
- Given "What is Rust?" is cached
- When invalidate is called for that prompt
- Then the entry is removed
- And subsequent lookups return miss

#### Scenario: Clear entire cache
- Given multiple entries are cached
- When clear is called
- Then all entries are removed
- And total_entries becomes 0

### Requirement: Cache Exclusions
The system MUST respect cache exclusion rules.

#### Scenario: Excluded intents not cached
- Given PrivacySensitive is in exclude_intents
- When a PrivacySensitive prompt response is returned
- Then the response is not stored in cache

#### Scenario: Short responses not cached
- Given min_response_length is 50
- When a response with 30 characters is returned
- Then the response is not stored in cache

### Requirement: Embedding Generation
The system MUST generate embeddings for semantic matching.

#### Scenario: Local embedding model used
- Given embedding_model is "bge-small-en-v1.5"
- When an embedding is generated
- Then a 384-dimensional vector is produced
- And no external API call is made

#### Scenario: Embedding is deterministic
- Given the same prompt text
- When embedding is generated multiple times
- Then the same vector is produced each time

### Requirement: Memory Limits
The system MUST respect configured memory limits.

#### Scenario: Max entries enforced
- Given max_entries is 10,000
- When the 10,001st entry is stored
- Then eviction occurs to maintain the limit

#### Scenario: Memory footprint acceptable
- Given 10,000 cached entries
- Then total memory usage is less than 100MB

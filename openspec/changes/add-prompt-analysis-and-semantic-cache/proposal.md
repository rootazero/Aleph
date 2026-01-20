# Change: Add Prompt Analysis Routing and Semantic Cache to Model Router

## Why

The Model Router (P0/P1) successfully implemented runtime metrics, health monitoring, retry/failover, and budget management. However, two P2 capabilities are needed for the next level of intelligence:

### Problem 1: Routing Based Only on TaskIntent, Not Prompt Content

Current routing decisions use:
- **TaskIntent** (CodeGeneration, ImageAnalysis, etc.) - requires caller to specify
- **Capability matching** - model must declare capabilities
- **Cost/latency tiers** - static configuration

**What's Missing**: No analysis of the actual prompt content. The system cannot:
- Detect prompt complexity (simple question vs. multi-step reasoning)
- Estimate token count for context window decisions
- Identify language patterns (Chinese, English, mixed, code)
- Recognize reasoning requirements (math, logic, chain-of-thought)
- Route based on code density or technical terminology

**Impact**: Suboptimal model selection - simple questions go to expensive models; complex reasoning tasks go to fast but less capable models.

### Problem 2: Repeated API Calls for Similar Prompts

Every request, even semantically identical ones, triggers a full API call:
- User asks "What is Rust?" → API call
- User asks "Tell me about Rust" → Another API call (same intent!)
- Slight rephrasing triggers new call even with cached exact match

**What's Missing**: Semantic similarity matching that can:
- Recognize similar questions with different wording
- Return cached responses for semantically equivalent prompts
- Reduce latency and cost for repeated queries
- Learn from user's common query patterns

**Impact**: 30-50% of queries could potentially be served from cache; wasted API costs and added latency.

## What Changes

### 1. Prompt Analyzer (NEW)

A pre-routing analysis layer that extracts features from prompt content:

```
┌─────────────────────────────────────────────────────────────────┐
│                      PromptAnalyzer                             │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ Tokenizer    │  │ Complexity   │  │   LanguageDetector     │ │
│  │ (count)      │  │ Scorer       │  │   (lang/code ratio)    │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ ReasoningDtc │  │ DomainDtc    │  │   PromptFeatures       │ │
│  │ (indicators) │  │ (technical)  │  │   (output struct)      │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **TokenEstimator**: Fast BPE-based token counting (tiktoken-rs)
- **ComplexityScorer**: Lexical density, sentence structure, nested logic
- **LanguageDetector**: Natural language (en/zh/ja) + programming language ratio
- **ReasoningDetector**: Math patterns, logic keywords, chain-of-thought markers
- **DomainDetector**: Technical terms, code blocks, API references
- **PromptFeatures**: Unified output struct for routing decisions

### 2. Semantic Cache (NEW)

A similarity-based caching layer using text embeddings:

```
┌─────────────────────────────────────────────────────────────────┐
│                     SemanticCacheManager                        │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ Embedder     │  │ VectorStore  │  │   SimilarityMatcher    │ │
│  │ (bge-small)  │  │ (sqlite-vec) │  │   (threshold)          │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ CacheEntry   │  │ TTLManager   │  │   EvictionPolicy       │ │
│  │ (response)   │  │ (expiry)     │  │   (LRU/LFU)            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **Embedder**: Local embedding model (bge-small-en-v1.5, 384 dimensions)
- **VectorStore**: SQLite with sqlite-vec extension for ANN search
- **SimilarityMatcher**: Configurable cosine similarity threshold (default 0.85)
- **CacheEntry**: Stores prompt embedding, response, model used, metadata
- **TTLManager**: Time-based expiration with per-entry TTL support
- **EvictionPolicy**: LRU with optional LFU for high-value entries

### 3. Integration with Existing Components

```
┌─────────────────────────────────────────────────────────────────┐
│                     OrchestratedRouter (P1)                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    [NEW] Pre-Route Layer                  │   │
│  │  1. SemanticCache.lookup(prompt) → Option<CachedResponse> │   │
│  │  2. PromptAnalyzer.analyze(prompt) → PromptFeatures       │   │
│  └──────────────────────────────────────────────────────────┘   │
│                               ↓                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                 Routing Decision (Enhanced)               │   │
│  │  - Use PromptFeatures to adjust TaskIntent inference      │   │
│  │  - Consider complexity for model tier selection           │   │
│  │  - Check token estimate vs model context window           │   │
│  └──────────────────────────────────────────────────────────┘   │
│                               ↓                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                 Post-Execute Layer [NEW]                  │   │
│  │  - SemanticCache.store(prompt, response, model, ttl)      │   │
│  │  - MetricsCollector.record() (existing)                   │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Impact

- **Affected specs**: `model-router` (new capability spec)
- **Affected code**:
  - `core/src/dispatcher/model_router/` - New modules: prompt_analyzer.rs, semantic_cache.rs
  - `core/src/dispatcher/model_router/orchestrated_router.rs` - Integration
  - `core/src/dispatcher/model_router/matcher.rs` - Feature-aware routing
  - `core/src/lib.rs` - UniFFI exports for cache stats
  - `platforms/macos/Aether/Sources/` - Cache statistics UI
- **Dependencies**:
  - `tiktoken-rs` - Token counting
  - `sqlite-vec` - Vector similarity search (already in project)
  - `fastembed-rs` or `candle` - Local embedding inference
  - `whichlang` - Language detection
- **Non-breaking**: All new APIs, existing routing unchanged

## Success Criteria

1. **Prompt Analysis**:
   - Token estimation within 5% of actual tiktoken count
   - Complexity scoring correlates with actual model performance
   - Language detection >95% accuracy for en/zh
   - Analysis latency <5ms for typical prompts

2. **Semantic Cache**:
   - Cache hit rate >30% for repeated usage patterns
   - Similarity threshold tunable without rebuild
   - Cache lookup latency <10ms
   - Memory footprint <100MB for 10K entries
   - Zero false positives (semantically different but matched)

# Memory Module Optimization Design

> Inspired by [memory-lancedb-pro](https://github.com/pinkpixel-dev/memory-lancedb-pro), an excellent LanceDB memory plugin for OpenClaw.

**Date**: 2026-02-24
**Status**: Approved
**Scope**: `core/src/memory/`

---

## Overview

Aleph's memory module already has a sophisticated cognitive architecture (ACMA, VFS, Knowledge Graph, DreamDaemon). This design introduces **retrieval quality improvements** inspired by memory-lancedb-pro's production-proven patterns, while preserving all existing capabilities.

### Design Constraints

| Constraint | Decision |
|-----------|----------|
| Embedding strategy | Local-first (fastembed), remote as optional |
| Reranking | Local only (cosine rerank, no external API) |
| Adaptive retrieval | Rule-driven (heuristic patterns, no LLM) |
| Architecture approach | Pipeline refactor (Approach B) |

---

## 1. Multi-Stage Scoring Pipeline

The core innovation: a configurable scoring pipeline that processes candidates after RRF fusion.

### ScoringStage Trait

```rust
/// A single stage in the scoring pipeline
pub trait ScoringStage: Send + Sync {
    fn name(&self) -> &str;
    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact>;
}

/// Context passed to each stage
pub struct ScoringContext {
    pub query: String,
    pub query_embedding: Option<Vec<f32>>,
    pub timestamp: i64,
    pub config: ScoringPipelineConfig,
}
```

### Pipeline Stages (executed in order, re-sorted between stages)

| # | Stage | Operation | Formula |
|---|-------|-----------|---------|
| 1 | **Cosine Rerank** | Blend cosine similarity with original score | `0.7 * original + 0.3 * cosine_sim` |
| 2 | **Recency Boost** | Additive boost for recent memories | `score += exp(-age_days/14) * 0.1` |
| 3 | **Importance Weight** | Scale by importance field | `score *= (0.7 + 0.3 * importance)` |
| 4 | **Length Normalization** | Penalize verbose entries | `score *= 1/(1 + 0.5*log2(len/500))` |
| 5 | **Time Decay** | Multiplicative penalty for old memories | `score *= 0.5 + 0.5*exp(-age_days/60)` |
| 6 | **Hard Min Score** | Discard low-scoring candidates | Drop if `score < 0.35` |
| 7 | **MMR Diversity** | Demote near-duplicate results | Cosine > 0.85 → demote |

### Key Design Decisions

- RRF Fusion stays in `hybrid_retrieval/` (candidate generation, not post-processing)
- Pipeline attaches after fusion, processes the merged candidate list
- Each stage is a pure function — no side effects, easy to unit test
- Stages can be disabled via config (e.g., `recency_weight: 0.0` disables recency boost)

### File Organization

```
core/src/memory/scoring_pipeline/
├── mod.rs              # ScoringPipeline assembler + ScoringStage trait
├── context.rs          # ScoringContext
├── config.rs           # ScoringPipelineConfig
├── stages/
│   ├── mod.rs
│   ├── cosine_rerank.rs
│   ├── recency_boost.rs
│   ├── importance_weight.rs
│   ├── length_normalization.rs
│   ├── time_decay.rs
│   ├── hard_min_score.rs
│   └── mmr_diversity.rs
```

---

## 2. Embedding Optimization

### 2.1 LRU Embedding Cache

```rust
pub struct EmbeddingCache {
    entries: Mutex<LinkedHashMap<String, CacheEntry>>,
    max_size: usize,           // default: 256
    ttl: Duration,             // default: 30 min
    hits: AtomicU64,
    misses: AtomicU64,
}

struct CacheEntry {
    vector: Vec<f32>,
    created_at: Instant,
}
```

Cache key: `sha256(task_type + ":" + text)` (first 24 chars), differentiating query vs passage embeddings.

### 2.2 Task-Aware Embedding

```rust
pub enum EmbeddingTask {
    Query,      // Retrieval query (user input)
    Passage,    // Storage document (memory content)
    Default,    // Backward compatible
}

impl SmartEmbedder {
    pub async fn embed_with_task(&self, text: &str, task: EmbeddingTask) -> Result<Vec<f32>>;
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
    pub async fn embed_passage(&self, text: &str) -> Result<Vec<f32>>;
}
```

Implementation: fastembed's `multilingual-e5-small` supports task differentiation via prefix (`"query: "` vs `"passage: "`). Remote providers pass task type via API parameters.

### Files

- Enhance `smart_embedder.rs`: integrate cache + task-aware methods
- New `embedding_cache.rs`: standalone LRU cache
- Enhance `embedding_provider.rs`: EmbeddingTask propagation

---

## 3. Adaptive Retrieval & Noise Filtering

### 3.1 Adaptive Retrieval Gate

Lightweight rule engine at pipeline entry to decide whether to perform memory retrieval:

```rust
pub struct AdaptiveRetrievalGate {
    config: AdaptiveRetrievalConfig,
}

pub struct AdaptiveRetrievalConfig {
    pub enabled: bool,
    pub min_length_cjk: usize,        // default: 6
    pub min_length_other: usize,      // default: 15
    pub skip_patterns: Vec<String>,   // "hello", "yes", "/commands"
    pub force_patterns: Vec<String>,  // "remember", "上次", "之前"
}

pub enum RetrievalDecision {
    Retrieve,
    Skip,
    ForceRetrieve,
}
```

Priority: ForcePattern > SkipPattern > LengthCheck

CJK detection: check for Unicode CJK Unified Ideographs range (`\u{4e00}-\u{9fff}`).

### 3.2 Noise Filter (Dual Defense)

**Storage layer** (during ingestion):
- Filter agent denials ("I can't", "I'm sorry")
- Filter system-generated content (`<tags>`)
- Filter pure emoji / punctuation

**Retrieval layer** (pipeline tail):
- Filter short meaningless results
- Runs as final post-processing step

```rust
pub struct NoiseFilter {
    config: NoiseFilterConfig,
}

pub struct NoiseFilterConfig {
    pub enabled: bool,
    pub min_content_length: usize,    // default: 10
    pub denial_patterns: Vec<String>,
    pub boilerplate_patterns: Vec<String>,
}

impl NoiseFilter {
    pub fn should_store(&self, content: &str) -> bool;
    pub fn filter_results(&self, results: Vec<ScoredFact>) -> Vec<ScoredFact>;
}
```

### Files

- New `adaptive_retrieval.rs`
- New `noise_filter.rs`

---

## 4. Storage & Lifecycle Improvements

### 4.1 Dual Decay Mechanism

Separate "boost new" from "penalize old":

| Mechanism | Operation | Formula | Default Half-life | Effect |
|-----------|-----------|---------|-------------------|--------|
| **Recency Boost** | Additive | `score += exp(-age_days/14) * 0.1` | 14 days | New memories get bonus score |
| **Time Decay** | Multiplicative | `score *= 0.5 + 0.5*exp(-age_days/60)` | 60 days | Old memories lose weight (floor 50%) |

Relationship with existing `strength` field:
- `strength` remains for DreamDaemon's physical decay (determines if memory is garbage-collected)
- Recency Boost / Time Decay are **retrieval-time scoring adjustments** — they don't modify stored data
- Separation of concerns: strength = storage-level survival, Decay = retrieval-level priority

### 4.2 Storage Deduplication

Vector similarity check before storing:

```rust
pub async fn check_duplicate(
    store: &MemoryBackend,
    embedding: &[f32],
    scope_filter: &SearchFilter,
    threshold: f32,  // default: 0.95
) -> Result<Option<String>>;
```

Behavior: if score > 0.95, skip storage (or optionally refresh timestamp).

### 4.3 Auto JSONL Backup

```rust
pub struct MemoryBackupService {
    store: MemoryBackend,
    backup_dir: PathBuf,        // ~/.aleph/backups/memory/
    max_backups: usize,         // default: 7
}
```

Triggered by DreamDaemon during idle time, once daily. Rolling 7-day window.

### Files

- Enhance `decay.rs`: separate Recency/Decay concepts
- Enhance `ingestion.rs`: add deduplication check
- New `backup.rs`: JSONL backup service
- Enhance `dreaming.rs`: add backup trigger

---

## 5. Integration & Configuration

### 5.1 Retrieval Data Flow

```
User Input
    │
    ▼
┌─────────────────────┐
│  AdaptiveGate       │── Skip → no retrieval
└────────┬────────────┘
         │ Retrieve
         ▼
┌─────────────────────┐
│  SmartEmbedder      │── Cache hit → return vector
│  + EmbeddingCache   │── Cache miss → embed_query() → cache → return
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  CandidateGenerator │── Vector ANN (top-20)
│  (existing hybrid)  │── FTS BM25 (top-20)
│                     │── RRF Fusion (k=60)
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  ScoringPipeline    │── CosineRerank → RecencyBoost → ImportanceWeight
│  (7 stages)         │── → LengthNorm → TimeDecay → HardMinScore → MMR
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  NoiseFilter        │── filter low-quality results
└────────┬────────────┘
         ▼
    Results → ContextComposer → Agent
```

### 5.2 Storage Data Flow

```
Agent Response
    │
    ▼
┌─────────────────────┐
│  NoiseFilter        │── should_store() = false → skip
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  SmartEmbedder      │── embed_passage() → vector
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  Deduplication      │── score > 0.95 → skip (or refresh timestamp)
└────────┬────────────┘
         ▼
┌─────────────────────┐
│  Ingestion          │── PII scrub → write to LanceDB
└─────────────────────┘
```

### 5.3 Configuration

```toml
[memory.scoring_pipeline]
enabled = true
rerank_blend = 0.3
recency_half_life_days = 14.0
recency_weight = 0.1
length_norm_anchor = 500
time_decay_half_life_days = 60.0
hard_min_score = 0.35
mmr_similarity_threshold = 0.85

[memory.adaptive_retrieval]
enabled = true
min_length_cjk = 6
min_length_other = 15

[memory.noise_filter]
enabled = true
min_content_length = 10

[memory.deduplication]
enabled = true
similarity_threshold = 0.95

[memory.embedding_cache]
max_size = 256
ttl_seconds = 1800

[memory.backup]
enabled = true
max_backups = 7
```

### 5.4 File Change Summary

| Action | File | Description |
|--------|------|-------------|
| **New** | `scoring_pipeline/mod.rs` | Pipeline assembler + ScoringStage trait |
| **New** | `scoring_pipeline/context.rs` | ScoringContext |
| **New** | `scoring_pipeline/config.rs` | ScoringPipelineConfig |
| **New** | `scoring_pipeline/stages/*.rs` | 7 scoring stages |
| **New** | `adaptive_retrieval.rs` | Adaptive retrieval gate |
| **New** | `noise_filter.rs` | Noise filter (dual defense) |
| **New** | `embedding_cache.rs` | LRU cache with TTL |
| **New** | `backup.rs` | JSONL backup service |
| **Modify** | `smart_embedder.rs` | Integrate cache + task-aware |
| **Modify** | `hybrid_retrieval/hybrid.rs` | Attach ScoringPipeline |
| **Modify** | `ingestion.rs` | Add noise filter + dedup |
| **Modify** | `decay.rs` | Separate Recency/Decay concepts |
| **Modify** | `dreaming.rs` | Add backup trigger |
| **Modify** | `config/types/memory.rs` | New config fields |

### 5.5 Testing Strategy

- Each ScoringStage: unit test (pure function, input→output)
- AdaptiveGate: test various input patterns (CJK/English/commands/triggers)
- NoiseFilter: test both storage and retrieval paths
- EmbeddingCache: test TTL expiry and LRU eviction
- Deduplication: test threshold boundaries
- Integration: full pipeline end-to-end test

---

## Appendix: Key Inspirations from memory-lancedb-pro

| Feature | memory-lancedb-pro | Aleph Adaptation |
|---------|-------------------|------------------|
| 8-stage scoring pipeline | TypeScript, config-driven | Rust ScoringStage trait, config-driven |
| Dual decay (recency + time) | Separate additive/multiplicative | Same concept, integrated with existing strength field |
| Adaptive retrieval | CJK-aware length thresholds | Same approach, rule-driven |
| Noise filtering | Dual (capture + retrieval) | Same dual defense |
| Task-aware embedding | Jina v5 task selectors | fastembed prefix-based |
| LRU embedding cache | 256 entries, 30min TTL | Same spec |
| Storage deduplication | Vector similarity > 0.95 | Same threshold |
| JSONL backup | Daily, 7-day rolling | Same spec, DreamDaemon-triggered |
| Cross-encoder rerank | Jina API (remote) | Local cosine rerank (no external dependency) |
| Multi-scope isolation | agent/global/custom/project/user | Already exists in Aleph (namespace/workspace) |

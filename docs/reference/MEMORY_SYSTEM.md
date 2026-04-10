# Memory System

> Facts database, hybrid retrieval, and context augmentation

---

## Overview

Aleph's memory system provides:
- **Facts Database**: SQLite + sqlite-vec for unified vector + metadata storage
- **Hybrid Retrieval**: Vector similarity (ANN) + BM25 full-text search via FactRetrieval + ScoringPipeline
- **Context Augmentation**: Inject relevant memories into prompts
- **Intelligent Compression**: Automatic session compaction with importance scoring
- **Context Arbitration**: Redundancy detection and token budget management

**Location**: `src/memory/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Memory System                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Ingestion Layer                         │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │ Fact       │  │  Session   │  │   Tool     │          │  │
│  │  │ Extractor  │  │  History   │  │  Results   │          │  │
│  │  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘          │  │
│  │        └───────────────┼───────────────┘                  │  │
│  └────────────────────────┼──────────────────────────────────┘  │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Storage Layer                           │  │
│  │  ┌──────────────────────────────────────────────────┐    │  │
│  │  │         SQLite + sqlite-vec (Unified)              │    │  │
│  │  │                                                    │    │  │
│  │  │  facts       │ graph_nodes │ graph_edges │ memories│    │  │
│  │  │  • content   │ • name      │ • relation  │ • input │    │  │
│  │  │  • embedding │ • kind      │ • weight    │ • embed │    │  │
│  │  │  • metadata  │ • aliases   │ • context   │ • anchor│    │  │
│  │  │  • FTS index │ • decay     │ • decay     │ • FTS   │    │  │
│  │  └──────────────────────────────────────────────────┘    │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                   Retrieval Layer                          │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Vector    │  │   BM25     │  │  Reranker  │          │  │
│  │  │  Search    │  │  Search    │  │ (Optional) │          │  │
│  │  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘          │  │
│  │        └───────────────┼───────────────┘                  │  │
│  │                        ▼                                  │  │
│  │              ┌─────────────────┐                          │  │
│  │              │ Hybrid Fusion   │                          │  │
│  │              │ (RRF scoring)   │                          │  │
│  │              └─────────────────┘                          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Facts Database

**Location**: `src/memory/store/` (SQLite + sqlite-vec backend)

> **Historical Note**: An earlier plan proposed migrating to LanceDB, but the production backend
> remains SQLite + sqlite-vec. All memory operations (facts, graph, search) use SQLite via
> `SqliteMemoryBackend`, exposed as `MemoryBackend` (= `Arc<SqliteMemoryBackend>`).

### Storage Architecture

SQLite + sqlite-vec provides unified storage with embedded vector indexes:

```
memory.db (SQLite)
├── facts            -- MemoryFact records with embeddings + FTS index
├── graph_nodes      -- Knowledge graph entity nodes
├── graph_edges      -- Knowledge graph relationships
└── compression_sessions -- Session compaction records
```

### Storage Traits

```rust
/// Layer 2: Compressed facts — CRUD, vector/text/hybrid search, VFS path operations
pub trait MemoryStore: Send + Sync {
    async fn insert_fact(&self, fact: &MemoryFact) -> Result<()>;
    async fn vector_search(&self, embedding: &[f32], dim_hint: u32,
                           filter: &SearchFilter, limit: usize) -> Result<Vec<ScoredFact>>;
    async fn hybrid_search(&self, params: &HybridSearchParams<'_>) -> Result<Vec<ScoredFact>>;
    // ... 17 total methods
}

/// Knowledge graph — node/edge management, entity resolution, temporal decay
pub trait GraphStore: Send + Sync {
    async fn upsert_node(&self, node: &GraphNode) -> Result<()>;
    async fn resolve_entity(&self, query: &str, context_key: Option<&str>) -> Result<Vec<ResolvedEntity>>;
    // ... 7 total methods
}

/// Dream pipeline persistence — daily insights, dream status
pub trait DreamStore: Send + Sync { /* ... */ }

/// Compression session persistence
pub trait CompressionStore: Send + Sync { /* ... */ }

/// Unified backend type
pub type MemoryBackend = Arc<SqliteMemoryBackend>;
```

### Fact Structure

```rust
pub struct MemoryFact {
    pub id: String,
    pub content: String,
    pub fact_type: FactType,
    pub embedding: Option<Vec<f32>>,
    pub source_memory_ids: Vec<String>,
    pub path: String,              // VFS path (e.g. "aleph://user/preferences/coding")
    pub parent_path: String,
    pub fact_source: FactSource,   // Extracted | Summary | Document | Manual
    pub content_hash: String,
    pub embedding_model: String,
    pub confidence: f32,
    pub is_valid: bool,
    pub specificity: FactSpecificity,
    pub temporal_scope: TemporalScope,
    // ... timestamps, invalidation fields
}
```

---

## Embedding

**Location**: `src/memory/embedding_provider.rs`, `src/memory/embedding_manager.rs`

All embeddings go through remote OpenAI-compatible APIs via `EmbeddingProvider` trait:

```rust
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
    fn provider_id(&self) -> &str;
}

pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,       // e.g., https://api.siliconflow.cn/v1
    api_key: String,
    model: String,          // e.g., BAAI/bge-m3
    dimension: usize,
    batch_size: usize,
}
```

### Provider Presets

| Preset | API Base | Model | Dimensions |
|--------|----------|-------|------------|
| SiliconFlow | `https://api.siliconflow.cn/v1` | BAAI/bge-m3 | 1024 |
| OpenAI | `https://api.openai.com/v1` | text-embedding-3-small | 1536 |
| Ollama | `http://localhost:11434/v1` | nomic-embed-text | 768 |
| Custom | User-defined | User-defined | User-defined |

### Multi-Dimension Support

SQLite-vec stores multiple vector columns (`vec_768`, `vec_1024`, `vec_1536`) allowing provider switching without data loss.

---

## Hybrid Retrieval

**Location**: `src/memory/hybrid_retrieval/`

Combines vector similarity and keyword search:

```rust
pub struct HybridRetrieval {
    embedder: Arc<dyn EmbeddingProvider>,
    database: MemoryBackend,
    strategy: RetrievalStrategy,
}

pub enum RetrievalStrategy {
    VectorOnly,
    KeywordOnly,
    Hybrid { vector_weight: f32 },  // default: 0.7
}
```

### Reciprocal Rank Fusion (RRF)

```
score(doc) = Σ 1 / (k + rank_i(doc))
```

Where:
- `k = 60` (constant)
- `rank_i` = rank in retrieval method i

### Search Flow

```
Query: "How to configure API keys?"
    │
    ▼
┌─────────────────────────────────────────┐
│ 1. Embed query                           │
│    embed("How to configure API keys?")   │
│    → [0.23, -0.15, 0.42, ...]           │
└─────────────────────────────────────────┘
    │
    ├─────────────────────────────────────┐
    │                                      │
    ▼                                      ▼
┌─────────────────────┐    ┌─────────────────────┐
│ Vector Search       │    │ BM25 Search         │
│ cosine_similarity   │    │ keyword matching    │
│                     │    │                     │
│ Top-K results       │    │ Top-K results       │
└─────────────────────┘    └─────────────────────┘
    │                                      │
    └─────────────────┬────────────────────┘
                      ▼
            ┌─────────────────────┐
            │ RRF Fusion          │
            │ Merge & rerank      │
            └─────────────────────┘
                      │
                      ▼
            ┌─────────────────────┐
            │ Reranker (Optional) │
            │ Cross-encoder       │
            └─────────────────────┘
                      │
                      ▼
              Final Results
```

---

## Context Augmentation

**Location**: `src/memory/augmentation.rs`

Inject relevant memories into agent prompts:

```rust
pub struct ContextAugmenter {
    retrieval: Arc<HybridRetrieval>,
    config: AugmentationConfig,
}

pub struct AugmentationConfig {
    /// Max facts to retrieve
    pub max_facts: usize,

    /// Minimum relevance score
    pub min_score: f32,

    /// Token budget for memories
    pub token_budget: usize,
}

impl ContextAugmenter {
    pub async fn augment(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Fact>> {
        // 1. Extract query from recent messages
        // 2. Retrieve relevant facts
        // 3. Filter by score and budget
        // 4. Format for prompt injection
    }
}
```

### Prompt Injection Format

```
<relevant_memories>
- API keys are configured in ~/.aleph/config.json under "providers" (source: session, 2 days ago)
- The user prefers Claude over GPT for code tasks (source: user, 1 week ago)
</relevant_memories>
```

---

## Session Compression

**Location**: `src/memory/compression.rs`

When session history exceeds token limit:

```rust
pub struct SessionCompressor {
    memory: Arc<MemorySystem>,
    config: CompressionConfig,
}

pub struct CompressionConfig {
    /// Token threshold to trigger compression
    pub threshold_tokens: usize,

    /// Target tokens after compression
    pub target_tokens: usize,

    /// Keep last N messages uncompressed
    pub keep_recent: usize,
}
```

### Compression Flow

```
Session History (10,000 tokens)
    │
    ▼
┌─────────────────────────────────────────┐
│ 1. Extract facts from old messages      │
│    LLM: "What facts should I remember?" │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 2. Store extracted facts                │
│    → Facts DB (with embeddings)         │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 3. Generate summary of old messages     │
│    LLM: "Summarize this conversation"   │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ 4. Replace old messages with summary    │
│    [summary] + [recent N messages]      │
└─────────────────────────────────────────┘
    │
    ▼
Compressed History (4,000 tokens)
```

---

## Memory Decay

**Location**: `src/memory/decay.rs`

Older, unused facts decay over time:

```rust
pub fn calculate_decay(fact: &Fact, now: DateTime<Utc>) -> f32 {
    let age_days = (now - fact.created_at).num_days() as f32;
    let base_decay = (-0.01 * age_days).exp();
    let access_boost = (fact.access_count as f32).ln_1p() * 0.1;

    (base_decay + access_boost).min(1.0)
}
```

### Cleanup

```rust
pub async fn cleanup_decayed_facts(
    db: &FactsDb,
    threshold: f32,  // e.g., 0.1
) -> Result<usize> {
    // Delete facts where decay_score < threshold
}
```

---

## Cognitive Memory Architecture (ACMA)

**Location**: `src/memory/composer.rs`, `src/memory/decay.rs`, `src/memory/dreaming.rs`

Aleph's memory system uses three orthogonal dimensions for each `MemoryFact`:

| Dimension | Field | Values | Purpose |
|-----------|-------|--------|---------|
| Abstraction | `layer` | L0Abstract / L1Overview / L2Detail | Granularity (abstract to detail) |
| Temperature | `tier` | Core / ShortTerm / LongTerm | Temporal lifecycle |
| Visibility | `scope` | Global / Workspace / Persona | Access isolation |

### Memory Tiers

| Tier | Behavior | Decay |
|------|----------|-------|
| **Core** | Injected into system prompt every request | Never decays |
| **ShortTerm** | Default for new facts. High fidelity, recent. | Ebbinghaus curve (configurable half-life) |
| **LongTerm** | Consolidated semantic knowledge. | Protected from decay |

### Memory Scopes

| Scope | Visibility | Use Case |
|-------|-----------|----------|
| **Global** | All personas, all workspaces | User preferences, API keys |
| **Workspace** | All personas in one workspace | Project architecture, TODOs |
| **Persona** | One specific persona only | Role-specific patterns, drafts |

### Context Composition

At session start, `ContextComposer` assembles context via a layered union:

1. **Core(Persona=P) + Core(Global)** → injected into system prompt
2. **Query(Global) + Query(Workspace=W) + Query(Persona=P)** → relevant memories

```rust
// Build Core filter
let core_filter = ContextComposer::build_core_filter(&req);
// Build retrieval filter (STM + LTM, no Core)
let retrieval_filter = ContextComposer::build_retrieval_filter(&req);
```

### Forgetting Curve

Facts track persistent `strength` (0.0-1.0), updated by `DreamDaemon`:

- **Decay**: Exponential based on time since last access (`update_strength()`)
- **Reinforcement**: Each retrieval hit boosts strength (`on_access()`)
- **Consolidation**: STM facts with strength ≥ threshold distilled into LTM
- **Pruning**: Facts with strength below threshold permanently deleted
- **Protection**: Core tier facts never decay or get pruned

### ACMA Configuration

```toml
[memory.consolidation_pipeline]
enabled = true
strength_threshold = 0.6     # STM minimum strength for consolidation
pruning_threshold = 0.1      # below this, permanent deletion
max_facts_per_run = 50       # batch size per Dream cycle
cooldown_days = 1             # minimum interval between checks
```

---

## Retention Policies

**Location**: `src/memory/retention.rs`

```rust
pub struct RetentionPolicy {
    /// Max age for session facts (days)
    pub session_max_age_days: u32,

    /// Max age for tool facts (days)
    pub tool_max_age_days: u32,

    /// User facts never expire
    pub user_facts_permanent: bool,

    /// Max total facts
    pub max_total_facts: usize,
}
```

---

## Memory Graph

**Location**: `src/memory/graph.rs`

The memory graph maintains lightweight entity nodes and relations used for disambiguation and
graph-assisted filtering. Entities are extracted from compressed facts and DreamDaemon summaries,
then stored in SQLite via the `GraphStore` trait.

SQLite tables:
- `graph_nodes` (entity nodes with decay scores)
- `graph_edges` (weighted relations between entities)

---

## DreamDaemon

**Location**: `src/memory/dreaming/`

DreamDaemon runs during idle windows via `DreamPipeline`, which chains composable stages.

### Daily Pipeline (5 stages)

| # | Stage | Purpose |
|---|-------|---------|
| 1 | `SummarizeStage` | Cluster recent memories, produce daily insight summary |
| 2 | `DriftDetectStage` | Detect semantic drift between new and existing facts |
| 3 | `ConsolidateStage` | Promote high-strength STM facts to LTM |
| 4 | `WikiLintStage` | Lint wiki pages for quality issues |
| 5 | `DecayStage` | Apply Ebbinghaus decay to memory facts and graph scores |

### Weekly Pipeline (6 stages)

Daily stages + `DeepSynthesisStage` (cross-cluster pattern discovery).

### Implemented but Not Yet Registered

- `WikiIngestStage` — ingest wiki page content into memory
- `TunnelDiscoveryStage` — discover hidden connections between facts

Daily insights are stored in `daily_insights` (via `DreamStore` trait) and can be queried by date.

---

## Additional Components

### ContextComptroller

**Location**: `src/memory/context_comptroller/`

Post-retrieval arbitration with redundancy detection and token budget management.

```rust
pub struct ContextComptroller {
    embedder: Arc<dyn EmbeddingProvider>,
    config: ContextComptrollerConfig,
}

pub struct ContextComptrollerConfig {
    /// Similarity threshold for redundancy (0.0-1.0)
    pub redundancy_threshold: f32,  // default: 0.95

    /// Token budget for context
    pub token_budget: usize,  // default: 2000

    /// Retention mode
    pub retention_mode: RetentionMode,  // default: Hybrid
}

pub enum RetentionMode {
    /// Prefer transcript over facts
    PreferTranscript,

    /// Prefer facts over transcript
    PreferFact,

    /// Keep both if budget allows
    Hybrid,
}
```

**Features**:
- **Redundancy Detection**: Identifies duplicate information using cosine similarity ≥ 0.95
- **Priority Sorting**: Orders results by relevance score (descending)
- **Budget Enforcement**: Ensures total tokens stay within budget
- **Graceful Degradation**: Drops lower-priority items when budget exceeded

### ValueEstimator

**Location**: `src/memory/value_estimator/`

Importance scoring for memory facts with hybrid LLM + keyword approach.

```rust
pub struct ValueEstimator {
    llm_scorer: Option<Arc<LlmScorer>>,
    config: ValueEstimatorConfig,
}

pub struct ValueEstimatorConfig {
    /// Enable LLM-based scoring
    pub use_llm: bool,  // default: false

    /// LLM weight in hybrid scoring (0.0-1.0)
    pub llm_weight: f32,  // default: 0.7

    /// Keyword weight in hybrid scoring
    pub keyword_weight: f32,  // default: 0.3
}
```

#### Signal Types

The estimator detects 8 types of signals:

| Signal | Description | Base Score |
|--------|-------------|------------|
| `UserPreference` | User likes/dislikes | 0.9 |
| `FactualInfo` | Facts, data, knowledge | 0.8 |
| `Decision` | Decisions made | 0.85 |
| `PersonalInfo` | Personal details | 0.9 |
| `Question` | Questions asked | 0.5 |
| `Answer` | Answers provided | 0.6 |
| `Greeting` | Greetings, pleasantries | 0.1 |
| `SmallTalk` | Casual conversation | 0.2 |

#### LLM-based Scoring

For more accurate importance estimation:

```rust
pub struct LlmScorer {
    provider: Arc<dyn AiProvider>,
    config: LlmScorerConfig,
}

impl LlmScorer {
    pub async fn score(&self, text: &str) -> Result<f32> {
        // Sends structured prompt to LLM
        // Returns importance score 0.0-1.0
    }
}
```

**Hybrid Scoring Formula**:
```
final_score = (llm_score * 0.7) + (keyword_score * 0.3)
```

### CompressionDaemon

**Location**: `src/memory/compression_daemon/`

Background scheduler for automatic memory compression during idle periods.

```rust
pub struct CompressionDaemon {
    config: CompressionDaemonConfig,
    last_activity: Arc<RwLock<Instant>>,
    is_running: Arc<AtomicBool>,
}

pub struct CompressionDaemonConfig {
    /// Check interval in seconds
    pub check_interval_secs: u64,  // default: 3600 (1 hour)

    /// Idle threshold in seconds
    pub idle_threshold_secs: u64,  // default: 300 (5 minutes)

    /// Enable daemon
    pub enabled: bool,  // default: true
}
```

**Features**:
- Periodic idle detection
- Activity tracking
- Configurable compression function
- Graceful shutdown

### memory_search Tool

**Location**: `src/builtin_tools/memory_search.rs`

AlephTool implementation that integrates all memory components.

```rust
pub struct MemorySearchTool {
    database: MemoryBackend,
    comptroller: Arc<ContextComptroller>,
}

// Tool parameters
pub struct MemorySearchParams {
    /// Search query
    pub query: String,

    /// Maximum results
    pub limit: Option<usize>,

    /// Minimum similarity score
    pub min_score: Option<f32>,
}

// Tool output
pub struct MemorySearchOutput {
    /// Deduplicated facts
    pub facts: Vec<Fact>,

    /// Transcript chunks (if any)
    pub transcripts: Vec<TranscriptChunk>,

    /// Tokens saved by deduplication
    pub tokens_saved: usize,
}
```

**Features**:
- Hybrid retrieval (vector + keyword)
- Automatic deduplication via ContextComptroller
- Token budget management
- Fallback to transcripts if no facts found

---

## Configuration

```toml
[memory]
enabled = true
max_context_items = 5
retention_days = 90
similarity_threshold = 0.7
excluded_apps = ["com.apple.keychainaccess", "com.agilebits.onepassword7"]

ai_retrieval_enabled = true
ai_retrieval_timeout_ms = 3000
ai_retrieval_max_candidates = 20
ai_retrieval_fallback_count = 3

compression_enabled = true
compression_idle_timeout_seconds = 300
compression_turn_threshold = 20
compression_interval_seconds = 3600
compression_batch_size = 50
conflict_similarity_threshold = 0.85
max_facts_in_context = 5
raw_memory_fallback_count = 3

[memory.context_comptroller]
enabled = true
redundancy_threshold = 0.95
token_budget = 2000
retention_mode = "Hybrid"  # PreferTranscript | PreferFact | Hybrid

[memory.value_estimator]
enabled = true
use_llm = false  # Enable for more accurate scoring
llm_weight = 0.7
keyword_weight = 0.3

[memory.dreaming]
enabled = true
idle_threshold_seconds = 900
window_start_local = "02:00"
window_end_local = "05:00"
max_duration_seconds = 600

[memory.graph_decay]
node_decay_per_day = 0.02
edge_decay_per_day = 0.03
min_score = 0.1

[memory.memory_decay]
half_life_days = 30.0
access_boost = 0.2
min_strength = 0.1
protected_types = ["personal"]
```

---

## Manual Test Checklist

- Set `memory.dreaming.enabled = true` and adjust the window to include the current time.
- Set `memory.dreaming.idle_threshold_seconds = 5`, wait for idle, and confirm a daily insight appears in `daily_insights`.
- Trigger user activity during a dream run and confirm `dream_status.last_status = cancelled`.
- Verify `graph_nodes`/`graph_edges` are updated after a successful run.
- Raise `memory.memory_decay.min_strength` temporarily and confirm older facts are pruned.

---

## Usage Examples

### Basic Memory Search

```rust
use alephcore::builtin_tools::MemorySearchTool;

let tool = MemorySearchTool::new(database, comptroller);
let params = MemorySearchParams {
    query: "What are my API preferences?".to_string(),
    limit: Some(5),
    min_score: Some(0.7),
};

let result = tool.execute(params).await?;
println!("Found {} facts, saved {} tokens",
    result.facts.len(),
    result.tokens_saved
);
```


---

## Best Practices

### Memory Ingestion

1. **Use Semantic Chunking** for long conversations to preserve context
2. **Set appropriate chunk sizes** (400 tokens default, adjust based on use case)
3. **Enable overlap** (80 tokens) to avoid losing information at boundaries

### Context Management

1. **Set realistic token budgets** (2000 tokens default for context)
2. **Use Hybrid retention mode** for balanced fact/transcript mix
3. **Adjust redundancy threshold** (0.95 default) based on precision needs

### Importance Scoring

1. **Enable LLM scoring** for critical applications requiring high accuracy
2. **Use keyword scoring** for cost-sensitive or high-throughput scenarios
3. **Tune hybrid weights** (70% LLM, 30% keyword) based on your data

### Performance Optimization

1. **Enable CompressionDaemon** for automatic background compression
2. **Set idle threshold** appropriately (5 minutes default)
3. **Monitor token savings** via ContextComptroller metrics
4. **Use batch operations** when processing multiple facts

---

## Performance Metrics

### Memory Overhead

| Component | Memory Usage | Notes |
|-----------|--------------|-------|
| ContextComptroller | ~5MB | In-memory deduplication cache |
| ValueEstimator | ~2MB | Signal detection only |
| ScoringPipeline | ~2MB | Configurable scoring stages |

### Latency

| Operation | Typical Latency | Notes |
|-----------|-----------------|-------|
| Memory Search | 50-200ms | Depends on result count |
| Hybrid Search (FactRetrieval) | 100-300ms | Vector + BM25 fusion |
| LLM Scoring | 500-2000ms | Per fact, cacheable |
| ScoringPipeline | 5-20ms | Post-retrieval scoring stages |

### Token Savings

- **ContextComptroller**: 20-40% reduction via deduplication
- **Semantic Chunking**: 10-15% better retrieval precision
- **Compression**: 30-50% session history reduction

---

## Troubleshooting

### High Memory Usage

**Symptom**: Memory usage grows over time

**Solutions**:
1. Enable memory decay: `memory.memory_decay.min_strength = 0.1`
2. Reduce retention days: `memory.retention_days = 30`
3. Lower token budget: `memory.context_comptroller.token_budget = 1000`

### Slow Memory Search

**Symptom**: Queries take > 1 second

**Solutions**:
1. Reduce max_context_items: `memory.max_context_items = 3`
2. Increase similarity threshold: `memory.similarity_threshold = 0.8`
3. Disable LLM scoring: `memory.value_estimator.use_llm = false`

### Missing Relevant Facts

**Symptom**: Important facts not retrieved

**Solutions**:
1. Lower similarity threshold: `memory.similarity_threshold = 0.6`
2. Increase max_context_items: `memory.max_context_items = 10`
3. Use RippleTask for broader exploration
4. Check redundancy threshold: `memory.context_comptroller.redundancy_threshold = 0.98`

---

## Pending Connection

The following modules are implemented but **not yet wired into production paths**:

| Module | Location | Description |
|--------|----------|-------------|
| **rerank/** | `src/memory/rerank/` | 5 cross-encoder reranking providers (Jina, Voyage, SiliconFlow, vLLM, Pinecone) |
| **query_expander** | `src/memory/query_expander.rs` | Chinese synonym expansion for improved recall |
| **Event sourcing** | `src/memory/events/` | `MemoryCommandHandler`, `EventProjector`, `MemoryTimeTraveler` — full event-sourced fact lifecycle |
| **RippleTask** | `src/memory/ripple/` | Multi-hop knowledge exploration via vector similarity traversal |
| **TranscriptIndexer** | `src/memory/transcript_indexer/` | Near-realtime conversation indexing with semantic chunking |

These are candidates for future integration once their production wiring is validated.

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - How memory is used
- [Gateway](GATEWAY.md) - Memory RPC methods
- [Tool System](TOOL_SYSTEM.md) - Memory tools documentation

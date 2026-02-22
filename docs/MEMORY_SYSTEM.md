# Memory System

> Facts database, hybrid retrieval, and context augmentation

---

## Overview

Aleph's memory system provides:
- **Facts Database**: LanceDB for unified vector + metadata storage (migrated from SQLite + sqlite-vec)
- **Hybrid Retrieval**: Vector similarity (ANN) + BM25 full-text search
- **Context Augmentation**: Inject relevant memories into prompts
- **Intelligent Compression**: Automatic session compaction with importance scoring
- **Transcript Indexing**: Near-realtime conversation indexing with semantic chunking
- **Context Arbitration**: Redundancy detection and token budget management
- **Knowledge Exploration**: Multi-hop traversal for related fact discovery
- **Contradiction Resolution**: Automatic detection and evolution tracking
- **User Profiling**: Frequency-based characteristic distillation

**Location**: `core/src/memory/`

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
│  │  │              LanceDB (Unified)                    │    │  │
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

**Location**: `core/src/memory/store/` (LanceDB backend)

> **Migration Note**: The storage layer was migrated from SQLite + sqlite-vec to LanceDB in Feb 2026.
> All memory operations (facts, sessions, graph, search) now use LanceDB via the `MemoryBackend` type alias.
> SQLite (`StateDatabase`) is retained only for resilience state management at `core/src/resilience/database/`.

### Storage Architecture

LanceDB provides unified columnar storage with embedded vector indexes:

```
memory.lance/
├── facts/          -- MemoryFact records with embeddings + FTS index
├── graph_nodes/    -- Knowledge graph entity nodes
├── graph_edges/    -- Knowledge graph relationships
└── memories/       -- Raw conversation memory entries (Layer 1)
```

### Storage Traits

```rust
/// Layer 2: Compressed facts
pub trait MemoryStore: Send + Sync {
    async fn insert_fact(&self, fact: &MemoryFact) -> Result<()>;
    async fn vector_search(&self, embedding: &[f32], dim_hint: u32,
                           filter: &SearchFilter, limit: usize) -> Result<Vec<ScoredFact>>;
    async fn hybrid_search(&self, embedding: &[f32], dim_hint: u32,
                           query_text: &str, ...) -> Result<Vec<ScoredFact>>;
    // ... 17 total methods
}

/// Knowledge graph
pub trait GraphStore: Send + Sync {
    async fn upsert_node(&self, node: &GraphNode) -> Result<()>;
    async fn resolve_entity(&self, query: &str, context_key: Option<&str>) -> Result<Vec<ResolvedEntity>>;
    // ... 7 total methods
}

/// Layer 1: Raw memories
pub trait SessionStore: Send + Sync {
    async fn insert_memory(&self, memory: &MemoryEntry) -> Result<()>;
    async fn search_memories(&self, embedding: &[f32], filter: &MemoryFilter, limit: usize) -> Result<Vec<MemoryEntry>>;
    // ... 10 total methods
}

/// Unified backend type
pub type MemoryBackend = Arc<LanceMemoryBackend>;
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

**Location**: `core/src/memory/smart_embedder.rs`

Local embedding using `fastembed`:

```rust
pub struct SmartEmbedder {
    model: EmbeddingModel,  // bge-small-zh-v1.5 (384 dim)
}

impl SmartEmbedder {
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Returns 384-dimensional vector
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // Batch embedding for efficiency
    }
}
```

### Model Selection

| Model | Dimensions | Size | Best For |
|-------|------------|------|----------|
| `bge-small-zh-v1.5` | 384 | 24MB | Chinese + English |
| `all-MiniLM-L6-v2` | 384 | 22MB | English only |

---

## Hybrid Retrieval

**Location**: `core/src/memory/hybrid_retrieval/`

Combines vector similarity and keyword search:

```rust
pub struct HybridRetrieval {
    embedder: Arc<SmartEmbedder>,
    db: Arc<FactsDb>,
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

**Location**: `core/src/memory/augmentation.rs`

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

**Location**: `core/src/memory/compression.rs`

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

**Location**: `core/src/memory/decay.rs`

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

## Retention Policies

**Location**: `core/src/memory/retention.rs`

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

**Location**: `core/src/memory/graph.rs`

The memory graph maintains lightweight entity nodes and relations used for disambiguation and
graph-assisted filtering. Entities are extracted from compressed facts and DreamDaemon summaries,
then stored in LanceDB via the `GraphStore` trait.

LanceDB tables:
- `graph_nodes` (entity nodes with decay scores)
- `graph_edges` (weighted relations between entities)

---

## DreamDaemon

**Location**: `core/src/memory/dreaming.rs`

DreamDaemon runs during idle windows to:
- Cluster recent memories (default lookback: 24h)
- Produce a daily insight summary
- Update graph nodes/edges from the summary
- Apply decay to memory facts and graph scores

Daily insights are stored in `daily_insights` and can be queried by date.

---

## Memory System Evolution (New)

The following components were added in the Memory System Evolution project to provide intelligent memory management and cognitive features.

### TranscriptIndexer

**Location**: `core/src/memory/transcript_indexer/`

Provides near-realtime conversation transcript indexing with semantic chunking.

```rust
pub struct TranscriptIndexer {
    database: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: TranscriptIndexerConfig,
}

pub struct TranscriptIndexerConfig {
    /// Chunk size in tokens
    pub chunk_size: usize,  // default: 400

    /// Overlap between chunks in tokens
    pub overlap: usize,  // default: 80

    /// Enable semantic boundary detection
    pub semantic_chunking: bool,  // default: false
}
```

#### Semantic Chunking

Advanced chunking that preserves semantic coherence using embedding-based boundary detection:

```rust
pub struct SemanticChunker {
    embedder: Arc<SmartEmbedder>,
    config: SemanticChunkerConfig,
}

pub struct SemanticChunkerConfig {
    /// Minimum chunk size in tokens
    pub min_chunk_size: usize,  // default: 100

    /// Maximum chunk size in tokens
    pub max_chunk_size: usize,  // default: 800

    /// Similarity threshold for boundaries (0.0-1.0)
    pub boundary_threshold: f32,  // default: 0.85
}
```

**How it works**:
1. Split text into sentences
2. Compute embeddings for each sentence
3. Calculate cosine similarity between adjacent sentences
4. Create boundaries where similarity < threshold
5. Merge small chunks to meet minimum size

### ContextComptroller

**Location**: `core/src/memory/context_comptroller/`

Post-retrieval arbitration with redundancy detection and token budget management.

```rust
pub struct ContextComptroller {
    embedder: Arc<SmartEmbedder>,
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

**Location**: `core/src/memory/value_estimator/`

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

**Location**: `core/src/memory/compression_daemon/`

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

### RippleTask

**Location**: `core/src/memory/ripple/`

Local knowledge exploration through multi-hop vector similarity traversal.

```rust
pub struct RippleTask {
    database: MemoryBackend,
    config: RippleConfig,
}

pub struct RippleConfig {
    /// Maximum hops from seed fact
    pub max_hops: usize,  // default: 3

    /// Facts to retrieve per hop
    pub facts_per_hop: usize,  // default: 5

    /// Minimum similarity threshold
    pub similarity_threshold: f32,  // default: 0.7
}
```

**How it works**:
1. Start with seed fact(s)
2. Find similar facts using vector search
3. Expand to next hop from discovered facts
4. Continue until max_hops reached
5. Return all discovered facts with hop distance

**Use cases**:
- Expand context from single fact to related knowledge network
- Discover connections between seemingly unrelated facts
- Build comprehensive context for complex queries

### Fact Evolution Chain

**Location**: `core/src/memory/evolution/`

Automatic contradiction detection and resolution with evolution tracking.

```rust
pub struct ContradictionDetector {
    provider: Arc<dyn AiProvider>,
    embedder: Arc<SmartEmbedder>,
}

pub struct EvolutionChain {
    database: MemoryBackend,
}

pub enum ResolutionStrategy {
    /// Keep newer fact, mark older as superseded
    PreferNewer,

    /// Keep fact with higher confidence
    PreferHigherConfidence,

    /// Create evolution chain linking both
    CreateEvolution,
}
```

**Features**:
- **LLM-driven Detection**: Uses AI to identify contradictions
- **Keyword Fallback**: Falls back to keyword matching if LLM unavailable
- **Evolution Tracking**: Maintains complete audit trail
- **Flexible Resolution**: Three strategies for handling conflicts

**Example**:
```
Fact A (2024-01-01): "User prefers Python"
Fact B (2024-06-01): "User prefers Rust"

→ Contradiction detected
→ Strategy: CreateEvolution
→ Result: Fact B supersedes Fact A, evolution chain created
```

### ConsolidationTask

**Location**: `core/src/memory/consolidation/`

User profile distillation through frequency analysis and categorization.

```rust
pub struct ConsolidationAnalyzer {
    database: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: ConsolidationConfig,
}

pub struct ConsolidationConfig {
    /// Minimum frequency score
    pub min_frequency_score: f32,  // default: 0.7

    /// Similarity threshold for consolidation
    pub similarity_threshold: f32,  // default: 0.9

    /// Lookback period in days
    pub lookback_days: u32,  // default: 90
}
```

**Frequency Scoring**:
```
frequency_score = (confidence * 0.7) + (recency * 0.3)
```

Where:
- `confidence`: Fact confidence score (0.0-1.0)
- `recency`: Time-based decay (1.0 for recent, 0.0 for old)

**Categories**:
- Preferences
- Plans
- Learning
- Projects
- Personal
- Other

**Output**: `UserProfile` with categorized high-frequency facts

### memory_search Tool

**Location**: `core/src/builtin_tools/memory_search.rs`

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
embedding_model = "bge-small-zh-v1.5"
max_context_items = 5
retention_days = 90
vector_db = "lancedb"              # LanceDB unified storage (default)
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

# Memory System Evolution features
[memory.transcript_indexer]
enabled = true
chunk_size = 400
overlap = 80
semantic_chunking = false

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

[memory.compression_daemon]
enabled = true
check_interval_secs = 3600  # 1 hour
idle_threshold_secs = 300   # 5 minutes

[memory.ripple]
enabled = true
max_hops = 3
facts_per_hop = 5
similarity_threshold = 0.7

[memory.evolution]
enabled = true
resolution_strategy = "CreateEvolution"  # PreferNewer | PreferHigherConfidence | CreateEvolution

[memory.consolidation]
enabled = true
min_frequency_score = 0.7
similarity_threshold = 0.9
lookback_days = 90
conflict_similarity_threshold = 0.85
max_facts_in_context = 5
raw_memory_fallback_count = 3

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

### Knowledge Exploration with RippleTask

```rust
use alephcore::memory::ripple::RippleTask;

let ripple = RippleTask::new(database, config);
let seed_facts = vec![fact_id];

// Explore 3 hops from seed fact
let discovered = ripple.explore(seed_facts, 3).await?;
println!("Discovered {} related facts across {} hops",
    discovered.len(),
    discovered.iter().map(|f| f.hop_distance).max().unwrap()
);
```

### Contradiction Detection

```rust
use alephcore::memory::evolution::{ContradictionDetector, ResolutionStrategy};

let detector = ContradictionDetector::new(provider, embedder);
let chain = EvolutionChain::new(database);

// Check for contradictions
if detector.detect_contradiction(&fact_a, &fact_b).await? {
    // Resolve using evolution chain
    chain.resolve(
        &fact_a,
        &fact_b,
        ResolutionStrategy::CreateEvolution
    ).await?;
}
```

### User Profile Distillation

```rust
use alephcore::memory::consolidation::ConsolidationAnalyzer;

let analyzer = ConsolidationAnalyzer::new(database, embedder, config);

// Analyze last 90 days
let profile = analyzer.analyze_user_profile(90).await?;

for (category, facts) in profile.categories {
    println!("{}: {} high-frequency facts", category, facts.len());
}
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

### Knowledge Exploration

1. **Start with 2-3 hops** for RippleTask to avoid over-expansion
2. **Set similarity threshold** ≥ 0.7 to maintain relevance
3. **Limit facts per hop** (5 default) to control result size

### Contradiction Management

1. **Use CreateEvolution** strategy to maintain audit trail
2. **Enable LLM detection** for nuanced contradictions
3. **Review evolution chains** periodically to understand knowledge changes

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
| TranscriptIndexer | ~10MB | Lazy loading, minimal overhead |
| ContextComptroller | ~5MB | In-memory deduplication cache |
| ValueEstimator | ~2MB | Signal detection only |
| RippleTask | ~20MB | Temporary during exploration |
| Evolution Chain | ~5MB | Lightweight tracking |

### Latency

| Operation | Typical Latency | Notes |
|-----------|-----------------|-------|
| Memory Search | 50-200ms | Depends on result count |
| Semantic Chunking | 100-300ms | Per 1000 tokens |
| LLM Scoring | 500-2000ms | Per fact, cacheable |
| Contradiction Detection | 1-3s | LLM-based, fallback available |
| RippleTask (3 hops) | 200-500ms | Vector search only |

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

### Contradictory Information

**Symptom**: Conflicting facts in results

**Solutions**:
1. Enable evolution chain: `memory.evolution.enabled = true`
2. Run contradiction detection periodically
3. Review and resolve conflicts manually
4. Use PreferNewer strategy for time-sensitive data

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - How memory is used
- [Gateway](GATEWAY.md) - Memory RPC methods
- [Tool System](TOOL_SYSTEM.md) - Memory tools documentation
- [Memory Evolution Summary](MEMORY_EVOLUTION_SUMMARY.md) - Implementation details

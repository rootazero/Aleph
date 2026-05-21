# Memory System Optimization Design

> Reference: memory-lancedb-pro (~/Workspace/memory-lancedb-pro)
> Date: 2026-03-14
> Approach: Modular refactoring + new capabilities (方案 B)

## Overview

Optimize Aleph's memory system by learning from memory-lancedb-pro's advanced features while preserving Aleph's existing strengths (event sourcing, knowledge graph, VFS, DreamDaemon). Four phases, each independently deliverable.

## Phase 1: Retrieval Enhancement

### 1.1 RRF Reciprocal Rank Fusion

**Replace**: weighted score fusion in `hybrid_retrieval/mod.rs`

**Algorithm**:
```
RRF_score(d) = Σ 1 / (k + rank_i(d))
```
- `k = 60` (standard constant)
- Rank vector and BM25 results separately by their scores
- Normalize fused scores to [0, 1]
- Extra 15% weight boost for BM25 hits (aligned with memory-lancedb-pro)

**Changes**:
- `hybrid_retrieval/mod.rs`: add `RrfFusion` strategy alongside existing `WeightedFusion`
- Config: `fusion_strategy: "rrf" | "weighted"`, default `"rrf"`

### 1.2 Cross-Encoder Rerank

**New module**: `src/memory/rerank/`

```
rerank/
├── mod.rs          // RerankProvider trait + RerankPipeline
├── jina.rs         // Jina rerank API
├── siliconflow.rs  // SiliconFlow rerank API
├── voyage.rs       // Voyage rerank API (top_k not top_n)
├── pinecone.rs     // Pinecone rerank API (Api-Key header)
└── vllm.rs         // vLLM local rerank
```

**Trait**:
```rust
#[async_trait]
pub trait RerankProvider: Send + Sync {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>>;
    fn provider_id(&self) -> &str;
}
```

**Scoring**: `final = 0.6 * rerank_score + 0.4 * rrf_score`

**Fallback**: 5s timeout → fall back to cosine similarity rerank

**Integration**: insert before `CosineRerankStage` in scoring pipeline as optional stage

### 1.3 Query Expansion

**New file**: `src/memory/query_expander.rs`

- Detect Chinese queries → inject synonyms for BM25 recall improvement
- Built-in synonym mapping table (no external dependency)
- Optional LLM expansion mode (R8: semantic understanding to LLM)

```rust
pub struct QueryExpander;
impl QueryExpander {
    pub fn expand(query: &str) -> ExpandedQuery {
        ExpandedQuery {
            original: query.to_string(),
            bm25_query: expanded_with_synonyms,
            vector_query: query.to_string(),
        }
    }
}
```

### 1.4 Retrieval Trace

**New file**: `src/memory/retrieval_trace.rs`

```rust
pub struct RetrievalTrace {
    pub query: String,
    pub timestamp: i64,
    pub stages: Vec<TraceStage>,
}
pub struct TraceStage {
    pub name: String,
    pub duration_ms: u64,
    pub input_count: usize,
    pub output_count: usize,
    pub scores: Vec<ScoreSnapshot>,
}
pub struct ScoreSnapshot {
    pub fact_id: String,
    pub score: f32,
    pub rank: usize,
}
```

- Each scoring pipeline stage accepts `Option<&mut RetrievalTrace>`
- Disabled in production by default, enabled via debug panel request
- New Gateway RPC: `memory.retrieve_with_trace`

---

## Phase 2: Lifecycle Improvements

### 2.1 Tiered Decay Model

**Replace**: global `DecayConfig` in `decay.rs` / `lazy_decay.rs`

```rust
pub struct TieredDecayConfig {
    pub core: TierDecayParams,        // Core tier: half_life=90d
    pub long_term: TierDecayParams,   // LongTerm tier: half_life=45d
    pub short_term: TierDecayParams,  // ShortTerm tier: half_life=7d
    pub protected_types: Vec<FactType>,
}
pub struct TierDecayParams {
    pub half_life_days: f32,
    pub min_strength: f32,
    pub reinforcement: AccessReinforcementConfig,
}
```

**Decay function** (keep Aleph's exponential, not memory-lancedb-pro's logistic):
```
strength = 0.5^(days_since_access / effective_half_life)
```

### 2.2 Access Reinforcement

**Replace**: simple linear `access_boost`

```rust
pub struct AccessReinforcementConfig {
    pub factor: f32,              // default: 0.5
    pub max_multiplier: f32,      // default: 3.0
    pub access_decay_days: f32,   // default: 30.0
}

fn effective_half_life(
    base: f32, access_count: u32,
    days_since_last_access: f32, config: &AccessReinforcementConfig,
) -> f32 {
    let freshness = (-days_since_last_access * LN_2 / config.access_decay_days).exp();
    let effective_count = access_count as f32 * freshness;
    let extension = base * config.factor * (1.0 + effective_count).ln();
    (base + extension).min(base * config.max_multiplier)
}
```

**Effect** (ShortTerm, base=7d): 0 access → 7d, 3 recent → 11.8d, cap at 21d.

### 2.3 Tier Promotion

**Integrated into DreamDaemon consolidation cycle**:

```rust
pub struct PromotionCriteria {
    pub short_to_long: PromotionRule,
    pub long_to_core: PromotionRule,
}
pub struct PromotionRule {
    pub min_access_count: u32,
    pub min_age_days: f32,
    pub min_strength: f32,
}
```

**Defaults**: ShortTerm→LongTerm (access≥3, age≥3d, strength≥0.5), LongTerm→Core (access≥10, age≥30d, strength≥0.7).

Produces `TierTransitioned` events (already exists).

---

## Phase 3: Reflection System

### 3.1 Session-End Reflection Trigger

**Integration**: Agent Loop session end signal

**Flow**:
```
session end → gate check (turns≥5, chars≥200, cooldown≥30min)
    → collect raw memories + extracted facts
    → LLM reflection (single call)
    → structured parse → 4 categories
    → write to FactStore (fact_source=Reflection)
    → emit FactCreated events
```

**Gate config**:
```rust
pub struct ReflectionGate {
    pub min_turns: u32,            // 5
    pub min_user_chars: u32,       // 200
    pub cooldown_minutes: u32,     // 30
}
```

### 3.2 Four-Category Structured Extraction

**New module**: `src/memory/reflection/`

```
reflection/
├── mod.rs       // ReflectionService
├── prompt.rs    // Prompt templates
├── parser.rs    // Markdown section parser
└── mapper.rs    // Parse results → MemoryFact mapping
```

**LLM output format** (structured Markdown):
```markdown
## Invariants
- {long-term preferences, work patterns, identity traits}

## Derived
- {new info learned this session, temporary context}

## Lessons
- {symptom}: {cause} → {fix/prevention}

## Open Loops
- {follow-up items with action verbs}
```

**Parsed structure**:
```rust
pub struct ReflectionOutput {
    pub invariants: Vec<String>,    // → Core tier, Personal/Preference
    pub derived: Vec<String>,       // → ShortTerm tier, Contextual
    pub lessons: Vec<LessonItem>,   // → LongTerm tier, Lesson (new FactType)
    pub open_loops: Vec<String>,    // → Daemon follow-up
}
pub struct LessonItem {
    pub symptom: String,
    pub cause: String,
    pub resolution: String,
}
```

**Tier/FactType mapping**:

| Category | Tier | FactType | Confidence | Half-life |
|----------|------|----------|------------|-----------|
| Invariants | Core | Personal/Preference | 0.85 | 90d |
| Derived | ShortTerm | Contextual | 0.70 | 7d |
| Lessons | LongTerm | Lesson (new) | 0.80 | 45d |
| Open Loops | not stored | — | — | — |

### 3.3 Open Loops → Daemon Integration

Open loops are actions, not memories:

```rust
pub struct OpenLoopAction {
    pub description: String,
    pub source_session_id: String,
    pub created_at: i64,
}
```

**Flow**: `OpenLoopDetected` event → DreamDaemon listens → next consolidation: LLM checks if resolved → unresolved: inject into system prompt as reminder → resolved: extract as Lesson.

### 3.4 Deduplication with Incremental Compression

1. Reflection prompt includes already-extracted facts summary → LLM focuses on cross-turn insights
2. Reflection output passes through existing `ConflictDetector` (similarity ≥ 0.85)
3. On conflict: reflection version wins (global perspective, higher quality), incremental version invalidated

---

## Phase 4: UI Integration

### 4.1 Extend Existing Memory Configuration Page

**Current**: 7 sections, 25+ settings. **Add 3 new sections + modify 1**:

```
MemoryView (extended)
├── [existing] BasicSettings
├── [existing] AIRetrievalSettings
├── [existing] CompressionSettings
├── [NEW] RetrievalPipelineSettings     ← RRF + query expansion
├── [NEW] RerankProviderSettings        ← cross-encoder config
├── [MODIFY] FactDecaySettings          ← tiered decay with tabs
├── [existing] GraphDecaySettings
├── [existing] DreamingSettings          ← add reflection sub-section
├── [existing] StorageBackupSettings
└── [NEW] RetrievalDebugPanel           ← collapsible trace viewer
```

### 4.2 New: Retrieval Pipeline Settings

- Fusion Strategy dropdown (RRF / Weighted)
- RRF Constant k (number)
- BM25 Bonus Weight (number)
- Enable Query Expansion (checkbox)
- Expansion Mode dropdown (Built-in Synonyms / LLM-Powered)

### 4.3 New: Rerank Provider Settings

- Enable Cross-Encoder Rerank (checkbox)
- Provider dropdown (Jina / SiliconFlow / Voyage / Pinecone / vLLM)
- API Base URL (text)
- API Key (password)
- Model (text)
- Timeout ms (number)
- Rerank Weight (number, default 0.6)
- [Test Connection] button → `memory.test_rerank_connection` RPC

### 4.4 Modified: Fact Decay Settings (Tiered)

Replace single parameter group with **3 tabs** (Core / LongTerm / ShortTerm):
- Half-Life days (per tier)
- Min Strength (per tier)
- Access Reinforcement Factor
- Max Half-Life Multiplier
- Access Decay days
- Protected Types (Core tab only)
- Tier Promotion criteria (promotion rules per tier transition)

### 4.5 Extended: Dreaming Settings

Add sub-section "Session Reflection":
- Enable Session-End Reflection (checkbox)
- Min Turns to Trigger (number)
- Min User Chars (number)
- Cooldown minutes (number)
- Enable Open Loop Tracking (checkbox)
- Inject to System Prompt (checkbox)

### 4.6 New: Retrieval Debug Panel

Collapsible panel at bottom of Memory page:
- Query input + Search button
- Pipeline Trace table (stage, items, time, top score)
- Result Details list (rank, score, content preview, tier, type, per-stage scores)
- RPC: `memory.retrieve_with_trace`

### 4.7 Config Schema Changes

All new fields use `#[serde(default)]` for backward compatibility. Existing `decay` field marked `#[deprecated]` with auto-migration to `tiered_decay` on startup.

New Gateway RPCs:
- `memory.test_rerank_connection` — test rerank provider connectivity
- `memory.retrieve_with_trace` — retrieval with full trace for debug panel

---

## Architecture Compliance

| Redline | Compliance |
|---------|-----------|
| R1 (Brain-Limb Separation) | All new modules in core, no platform APIs |
| R2 (UI in Leptos) | All UI in Panel WASM, no Tauri business logic |
| R4 (I/O-Only Interfaces) | Gateway only passes JSON-RPC, no business logic |
| R8 (LLM Sovereignty) | Reflection extraction, Open Loop resolution, query expansion (LLM mode) all delegate to LLM |
| R9 (Everything is a Tool) | Rerank/decay/reflection config manageable via tools |
| R10 (Intelligence in Prompts) | Reflection intelligence in prompt template, not code |

## Summary

| Phase | Scope | Key Files |
|-------|-------|-----------|
| P1 | Retrieval | `hybrid_retrieval/`, `rerank/`, `query_expander.rs`, `retrieval_trace.rs` |
| P2 | Lifecycle | `decay.rs`, `lazy_decay.rs`, DreamDaemon promotion logic |
| P3 | Reflection | `reflection/` (new module), `dreaming.rs` integration |
| P4 | UI | `panel/src/views/settings/memory.rs`, Gateway handlers |

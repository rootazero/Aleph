# Dream Consolidation Enhancement Design

> Learn from OpenClaw, surpass OpenClaw — leverage Aleph's Rust architecture and knowledge graph advantages.

## Background

OpenClaw's memory-core plugin implements a sophisticated "dreaming" mechanism with three sleep phases (Light/REM/Deep), recall signal tracking, and a 6-dimensional weighted scoring system for memory promotion. Aleph already has a DreamDaemon with a 6-stage pipeline, knowledge graph, DBSCAN clustering, and tiered decay — but lacks recall signal tracking, multi-dimensional promotion scoring, and dream auditing.

This design enhances Aleph's dreaming mechanism by adding the data foundation (recall signals), upgrading the intelligence (8-dimensional scoring), and providing observability (dream reports) — while preserving Aleph's existing architectural strengths.

## Goals

1. **Track retrieval signals** — record every memory search hit for later analysis
2. **8-dimensional promotion scoring** — replace simple `access_count >= 2` rule with weighted multi-dimensional evaluation
3. **Leverage Aleph's unique advantages** — graph_centrality and cluster_cohesion dimensions that OpenClaw cannot achieve
4. **Synthesis promotion** — condense clusters of ShortTerm facts into new high-level LongTerm facts via LLM
5. **Layered retrieval** — synthesized facts as stable background context, regular facts as dynamic detail
6. **Dream audit reports** — structured reports for debugging and observability
7. **Clean code** — remove replaced logic, no dead code accumulation

## Non-Goals

- Dream narrative/diary generation (no extra LLM calls for aesthetics)
- Phase signal passing between pipeline stages (ConsolidateStage queries data directly)
- Changing DreamGate, DecayStage, ClusterStage, DriftDetectStage, or SummarizeStage
- Exposing dream reports as user-facing tools
- Modifying retrieval scoring formula with source-based weights (layered presentation instead)

---

## Part 1: Recall Signal Tracking

### New Table: `recall_signals`

```sql
CREATE TABLE recall_signals (
    id TEXT PRIMARY KEY,
    fact_id TEXT NOT NULL REFERENCES memory_facts(id),
    query_hash TEXT NOT NULL,
    query_text TEXT NOT NULL,
    channel TEXT NOT NULL DEFAULT 'unknown',
    score REAL NOT NULL,
    session_id TEXT,
    namespace TEXT NOT NULL DEFAULT 'owner',
    created_at INTEGER NOT NULL,
    day_bucket TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_recall_dedup
    ON recall_signals(fact_id, query_hash, day_bucket, channel);
```

### Deduplication Rule

`(fact_id, query_hash, day_bucket, channel)` unique constraint. Same query, same day, same channel = one signal. Different channels each count separately — cross-channel recall is a stronger signal (Aleph multi-endpoint advantage).

### Integration Point

After `FactStore::search_facts()` returns results, asynchronously write recall signals via `tokio::spawn`. Non-blocking — retrieval latency is unchanged.

```rust
// In memory retrieval path
let results = fact_store.search_facts(&query, &opts).await?;

// Fire-and-forget signal recording
let signal_store = signal_store.clone();
tokio::spawn(async move {
    if let Err(e) = signal_store.record_signals(&query, &channel, &results).await {
        tracing::warn!("recall signal recording failed: {e}");
    }
});
```

### New File

`src/memory/store/sqlite/recall_signals.rs` — RecallSignalStore with:
- `record_signals()` — batch insert with ON CONFLICT ignore (dedup)
- `aggregate_for_facts()` — single SQL query returning per-fact aggregates
- `cleanup_old_signals()` — prune signals older than configurable retention (default 90 days)

---

## Part 2: 8-Dimensional Promotion Scoring

### Dimensions

| Dimension | Weight | Source | Formula |
|-----------|--------|--------|---------|
| frequency | 0.20 | recall_signals count | `log1p(signal_count) / log1p(10)` |
| relevance | 0.22 | recall_signals avg score | `total_score / signal_count` |
| diversity | 0.13 | unique queries + channels | `max(unique_queries, unique_channels) / 5` |
| recency | 0.12 | last_accessed_at | `exp(-ln2 / 14.0 * days_since_access)` |
| consolidation | 0.10 | day_bucket span | `0.55 * spacing + 0.45 * span_days / 7` |
| conceptual | 0.05 | fact_type + path tags | `tag_count / 6` |
| graph_centrality | 0.10 | graph_edges count | `log1p(edge_count) / log1p(8)` |
| cluster_cohesion | 0.08 | DBSCAN cluster result | `1.0 - (dist_to_centroid / cluster_radius)` |

### Weight Rationale

- **relevance** highest (0.22): direct quality signal — how well the memory matched real queries
- **frequency** second (0.20): usage frequency indicates practical value
- **diversity** + **graph_centrality** medium: cross-scenario and cross-entity connectivity
- **recency** + **consolidation** lower: time factors already handled by decay engine, avoid double-counting
- **conceptual** + **cluster_cohesion** auxiliary: structural signals, not dominant

### Scoring Formula

```rust
pub struct PromotionScorer {
    weights: [f32; 8],
    thresholds: PromotionThresholds,
}

impl PromotionScorer {
    pub fn score(&self, dims: &[f32; 8]) -> f32 {
        self.weights.iter().zip(dims).map(|(w, d)| w * d).sum()
    }

    pub fn should_promote(&self, fact: &MemoryFact, score: f32, signal_count: u32, unique_queries: u32) -> bool {
        fact.tier == MemoryTier::ShortTerm
            && score >= self.thresholds.min_score
            && signal_count >= self.thresholds.min_signal_count
            && unique_queries >= self.thresholds.min_unique_queries
            && fact.age_hours() >= self.thresholds.min_age_hours
    }
}
```

### Thresholds

```rust
pub struct PromotionThresholds {
    min_score: f32,           // 0.65
    min_signal_count: u32,    // 3
    min_unique_queries: u32,  // 2
    min_age_hours: u64,       // 24
}
```

### New File

`src/memory/consolidation/promotion_scorer.rs` — PromotionScorer with dimension calculations, scoring, and threshold checking.

---

## Part 3: ConsolidateStage Rewrite

### Current Flow (to be replaced)

```
for fact in short_term_facts:
    if access_count >= 2 && strength >= threshold:
        promote to LongTerm
```

### New Flow

```
ConsolidateStage::run(ctx: &mut DreamContext)
  1. Collect ShortTerm facts (exclude age < 24h)
  2. Batch query recall_signals aggregates (single SQL, no N+1)
  3. Batch query graph_edges entity counts (single SQL)
  4. Read ClusterStage results from ctx.clusters
  5. Compute 8 dimensions per fact
  6. PromotionScorer.score() + should_promote()
  7. Batch update tier = LongTerm for promoted facts
  8. Record promotion events in ctx.promotions
```

### Aggregation SQL (single query, no N+1)

```sql
SELECT fact_id,
       COUNT(*) as signal_count,
       SUM(score) as total_score,
       COUNT(DISTINCT query_hash) as unique_queries,
       COUNT(DISTINCT channel) as unique_channels,
       COUNT(DISTINCT day_bucket) as recall_days,
       MIN(created_at) as first_recall,
       MAX(created_at) as last_recall
FROM recall_signals
WHERE fact_id IN (?, ?, ...)
GROUP BY fact_id
```

### DreamContext Enhancement

```rust
pub struct DreamContext {
    pub facts: Vec<MemoryFact>,
    pub clusters: Option<Vec<Cluster>>,       // from ClusterStage
    pub drift_report: Option<DriftReport>,    // from DriftDetectStage
    pub promotions: Vec<PromotionEvent>,      // from ConsolidateStage (NEW)
    pub decay_stats: Option<DecayStats>,      // from DecayStage
}
```

### Code to Delete

- `should_consolidate()` function in `consolidation/analyzer.rs`
- Any inline consolidation logic in the old ConsolidateStage that uses only access_count + strength

---

## Part 4: Dream Report System

### New Table: `dream_reports`

```sql
CREATE TABLE dream_reports (
    id TEXT PRIMARY KEY,
    pipeline_type TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    facts_collected INTEGER NOT NULL DEFAULT 0,
    clusters_found INTEGER NOT NULL DEFAULT 0,
    drift_detected INTEGER NOT NULL DEFAULT 0,
    drift_summary TEXT,
    candidates_evaluated INTEGER NOT NULL DEFAULT 0,
    facts_promoted INTEGER NOT NULL DEFAULT 0,
    promotion_details TEXT,
    facts_decayed INTEGER NOT NULL DEFAULT 0,
    facts_pruned INTEGER NOT NULL DEFAULT 0,
    nodes_decayed INTEGER NOT NULL DEFAULT 0,
    edges_decayed INTEGER NOT NULL DEFAULT 0,
    errors TEXT,
    namespace TEXT NOT NULL DEFAULT 'owner'
);
```

### Report Generation

DreamPipeline writes a report after all stages complete, aggregating stats from DreamContext.

### Audit Queries

```rust
impl DreamReportStore {
    async fn recent_reports(&self, limit: usize) -> Vec<DreamReport>;
    async fn promotion_history(&self, fact_id: &str) -> Vec<PromotionEvent>;
    async fn health_summary(&self) -> DreamHealthSummary;
}
```

### New File

`src/memory/store/sqlite/dream_reports.rs`

---

## Part 5: Enhance DeepSynthesisStage

### Current State

Aleph already has `DeepSynthesisStage` in `src/memory/dreaming/stages/synthesis.rs` that:
- Runs **only during weekly** dream cycles (`run_type == Weekly`)
- Groups LongTerm facts by `fact_type`, runs DBSCAN clustering
- Creates new `FactSource::Synthesis` facts with `tier=Core`, `layer=L0Abstract`
- **Problem 1**: Content is naive concatenation (`join(";")` with `"Pattern: "` prefix) — no LLM semantic condensation
- **Problem 2**: Only runs weekly — daily clusters from ConsolidateStage are wasted
- **Problem 3**: No deduplication — repeated weekly runs can generate redundant synthesis facts
- **Problem 4**: `build_synthesis_prompt()` already exists but is not wired to an LLM call

### Existing Infrastructure (no changes needed)

```rust
// Already defined in context/enums.rs:
pub enum FactSource {
    Extracted,           // LLM-extracted from conversation
    Summary,             // L1 Overview
    Document,            // User-uploaded
    Manual,              // User-created
    SessionCompressed,   // SessionCompactor
    Synthesis,           // ← Dream synthesis (ALREADY EXISTS)
}
```

The `source` column already exists in `memory_facts`. No schema migration needed for this part.

### Changes to DeepSynthesisStage

**Change 1: Wire LLM synthesis** — Replace the naive `format!("Pattern: {}", combined)` with an actual LLM call using the existing `build_synthesis_prompt()`.

```rust
// Current (to replace):
let theme = format!("[{}] {}", fact_type_str, combined);

// New:
let prompt = build_synthesis_prompt(&cluster_facts_tuples);
let response = provider.complete(&prompt).await?;
let synthesized_content = parse_synthesis_response(&response)?;
```

**Change 2: Enable daily synthesis** — Add a second trigger path: during daily runs, ConsolidateStage can invoke synthesis for clusters that meet recall signal thresholds (not just weekly DBSCAN).

```rust
// In DeepSynthesisStage::should_run():
// Old: ctx.run_metadata.run_type == DreamRunType::Weekly
// New: always true — but daily runs use ConsolidateStage clusters,
//      weekly runs use full LTM re-clustering (existing behavior)
async fn should_run(&self, ctx: &DreamContext) -> bool {
    true  // daily: synthesize from ctx.clusters; weekly: full LTM scan
}
```

**Change 3: Deduplication with refresh** — Before inserting, check vector similarity against existing Synthesis facts. If near-duplicate found, skip insertion but refresh its `updated_at` to delay decay (confirms it's still valid).

```rust
// Before insert:
let existing = ctx.database.search_facts_by_source(
    FactSource::Synthesis, namespace, limit: 50
).await?;
let near_dup = existing.iter().find(|e| {
    e.embedding.as_ref().map_or(false, |emb| {
        cosine_similarity(&synthesized_embedding, emb) > 0.85
    })
});
if let Some(dup) = near_dup {
    // Refresh timestamp — confirms this synthesis is still valid
    ctx.database.touch_fact_updated_at(&dup.id).await?;
    continue; // skip redundant insertion
}
```

**Change 4: Do NOT modify source facts' specificity** — Remove the existing code (lines 186-199) that sets source facts to `FactSpecificity::Abstract` after synthesis. Source facts must retain their original specificity to remain fully visible in retrieval.

```rust
// DELETE this block from current DeepSynthesisStage::execute():
// for source_id in &insight.source_fact_ids {
//     source_fact.specificity = FactSpecificity::Abstract;
//     ...
// }
```

Rationale: Synthesis facts are additive (new Core facts), not a replacement. Source facts continue to serve as query-relevant detail in Layer 2 retrieval. Modifying their specificity would reduce their visibility and cause information loss through double-distillation.

**Change 5: Add synthesis thresholds for daily runs**

```rust
pub struct SynthesisThresholds {
    min_cluster_size: usize,      // 3
    min_avg_score: f32,           // 0.50 — cluster average promotion score
    min_recall_signals: u32,      // 5 — total signals across cluster members
}
```

### Cost Control

- Daily: 0-3 LLM calls (only qualifying clusters from ConsolidateStage)
- Weekly: same as current (full LTM re-clustering), but with LLM instead of concatenation
- Each call: small cluster (3-10 facts), ~500-1000 tokens input

---

## Part 6: Layered Retrieval

### Concept

Synthesized facts (source=Synthesis) and regular facts (source=Extraction) serve different roles in context building. Instead of mixing them with score-based weights, present them in separate layers.

### Retrieval Architecture

```
Memory Context Assembly
  ├─ Layer 1: Background Context (stable)
  │   └─ Top-K synthesized facts (source=Synthesis, tier=LongTerm)
  │   └─ Injected into system prompt as "What I know about you"
  │   └─ Selection: by strength + recency, limit 5-10
  │
  └─ Layer 2: Query-Relevant Detail (dynamic)
      └─ Hybrid search results (all sources, all tiers)
      └─ Injected as retrieval context per query
      └─ Selection: by vector + FTS relevance, limit 10-20
```

### Implementation

The retrieval path gains a two-phase assembly:

```rust
pub struct MemoryContext {
    pub background: Vec<MemoryFact>,   // Layer 1: synthesized, stable
    pub relevant: Vec<MemoryFact>,     // Layer 2: query-matched, dynamic
}

impl MemoryRetriever {
    pub async fn build_context(&self, query: &str, namespace: &str) -> MemoryContext {
        // Layer 1: top synthesized facts (cheap query, cached between turns)
        let background = self.fact_store
            .top_synthesized(namespace, limit: 10)
            .await?;

        // Layer 2: hybrid search (existing path, unchanged)
        let relevant = self.fact_store
            .search_facts(query, &opts)
            .await?;

        MemoryContext { background, relevant }
    }
}
```

### Layer 1 Caching

Background context changes only after dream cycles. Cache it per-session and invalidate when `dream_reports` table gets a new entry. This avoids repeated queries for stable data.

### Prompt Integration

```
[System Prompt]
## Long-term knowledge about the user
{background facts, joined by newlines}

## Relevant memories for this conversation
{relevant facts from hybrid search}
```

### No Scoring Formula Changes

Retrieval scoring (vector + FTS hybrid) remains unchanged. No source-based weight hacking. The separation is purely at the presentation layer.

---

## Part 7: Cleanup and Migration

### Files to Create

| File | Responsibility |
|------|---------------|
| `src/memory/store/sqlite/recall_signals.rs` | RecallSignalStore: signal write, aggregate query, dedup, cleanup |
| `src/memory/store/sqlite/dream_reports.rs` | DreamReportStore: report write, query, health summary |
| `src/memory/consolidation/promotion_scorer.rs` | PromotionScorer: 8-dimensional scoring engine |

### Files to Modify

| File | Change |
|------|--------|
| `src/memory/store/sqlite/mod.rs` | Add migrations for new tables (recall_signals, dream_reports), expose new stores |
| `src/memory/store/sqlite/facts.rs` | Add `top_synthesized()` query for Layer 1 retrieval |
| `src/memory/store/mod.rs` | Add recall_signals() and dream_reports() to MemoryBackend |
| `src/memory/consolidation/analyzer.rs` | Delete `should_consolidate`, rewrite ConsolidateStage with promotion scoring |
| `src/memory/dreaming/stages/synthesis.rs` | Wire LLM synthesis, enable daily runs, add dedup check |
| `src/memory/mod.rs` | Hook signal tracking into retrieval path, implement layered MemoryContext assembly |
| `src/memory/store/sqlite/graph.rs` | Add `edge_count_for_entities()` batch query |

### Unchanged Code

- DreamGate 3-level gate chain
- DreamPipeline stage orchestration framework
- DecayStage + LazyDecayEngine
- ClusterStage / DriftDetectStage / SummarizeStage
- Knowledge graph store (graph.rs) — only add query method
- Conflict detection + LLM arbitration
- Ripple exploration
- Hybrid search scoring (vector + FTS)

### Migration

1. **New tables**: `recall_signals`, `dream_reports` — pure additive
2. **No schema changes to existing tables** — `source` column and `FactSource::Synthesis` already exist in `memory_facts`
3. **No data migration**: Existing ShortTerm facts re-evaluated with new scoring on next dream cycle

---

## Architecture Alignment

| Principle | How This Design Complies |
|-----------|------------------------|
| R3 Core Minimalism | No new heavy dependencies — pure Rust computation + SQLite |
| R8 LLM Sovereignty | Scoring is deterministic (high-frequency per-fact); LLM reserved for synthesis (low-frequency, high-value condensation) |
| R10 Intelligence in Prompt | Synthesized facts inject long-term knowledge into system prompt, enriching LLM's context without middleware |
| P1 Low Coupling | ConsolidateStage queries stores directly; layered retrieval is presentation-layer separation, not scoring coupling |
| P2 High Cohesion | Each new file has single responsibility; reuses existing FactSource::Synthesis |
| P4 Dependency Inversion | PromotionScorer depends on data traits, not concrete stores |
| P6 Simplicity | 3 new files, delete old consolidation logic, no speculative abstractions |
| P7 Defensive Design | Signal recording is fire-and-forget; synthesis dedup prevents redundant facts; Layer 1 cache avoids repeated queries |

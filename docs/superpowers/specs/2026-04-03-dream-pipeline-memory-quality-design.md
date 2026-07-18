# Dream Pipeline & Memory Consolidation Quality

**Date:** 2026-04-03
**Status:** Approved
**Scope:** Category A — Memory consolidation quality improvements

## Background

Aleph's DreamDaemon and SessionCompactor provide hierarchical memory compression (d0/d1/d2) and idle-time consolidation. Analysis of Claude Code's AutoDream mechanism and OpenClaw's context engine revealed several quality gaps:

1. **Simplistic clustering** — Current `cluster_memories()` groups by `window_title` only, missing semantic relationships
2. **No identifier preservation** — Summaries may shorten/paraphrase UUIDs, file paths, URLs
3. **No drift detection** — Stale facts coexist with newer contradicting facts indefinitely
4. **No cross-session synthesis** — DreamDaemon only processes 24h of memories within a namespace, never extracting higher-order patterns from long-term memory

## Decision: Pipeline Architecture (方案 B)

Refactor `DreamDaemon::run_dream()` into a staged pipeline with independent, composable, testable stages. Each stage implements a `DreamStage` trait.

Rejected alternatives:
- **方案 A (渐进增强):** Would grow `run_dream()` into a monolithic function, violating P2 (High Cohesion)
- **方案 C (独立 Cortex 服务):** Introduces unnecessary concurrency complexity over shared MemoryStore, violating P6 (Simplicity)

---

## Architecture

### DreamStage Trait

```rust
#[async_trait]
pub trait DreamStage: Send + Sync {
    /// Stage name for logging and metrics
    fn name(&self) -> &'static str;

    /// Whether this stage should run in the current dream cycle
    async fn should_run(&self, ctx: &DreamContext) -> bool { true }

    /// Execute stage logic, consuming and producing DreamContext
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext>;
}
```

### DreamContext — Pipeline Shared State

```rust
pub struct DreamContext {
    pub memories: Vec<MemoryEntry>,
    pub clusters: Vec<MemoryCluster>,
    pub new_facts: Vec<MemoryFact>,
    pub drift_resolutions: Vec<DriftAction>,
    pub config: DreamingConfig,
    pub run_metadata: DreamRunMetadata,
    pub activity_checker: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub struct DreamRunMetadata {
    pub run_type: DreamRunType,
    pub last_daily_at: Option<i64>,
    pub last_weekly_at: Option<i64>,
    pub cycle_id: String,
}

pub enum DreamRunType { Daily, Weekly }
```

### Pipeline Executor

```rust
pub struct DreamPipeline {
    stages: Vec<Box<dyn DreamStage>>,
}

impl DreamPipeline {
    pub fn daily() -> Self {
        Self::new()
            .stage(CollectStage)
            .stage(ClusterStage)
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(DecayStage)
    }

    pub fn weekly() -> Self {
        Self::daily()
            .stage(DeepSynthesisStage)
    }

    pub async fn run(&self, mut ctx: DreamContext) -> Result<DreamReport> {
        for stage in &self.stages {
            if !stage.should_run(&ctx).await { continue; }
            if (ctx.activity_checker)() {
                return Ok(DreamReport::interrupted(ctx, stage.name()));
            }
            ctx = stage.execute(ctx).await?;
        }
        Ok(DreamReport::completed(ctx))
    }
}
```

### Scheduling

- **Daily pipeline:** Existing DreamDaemon schedule (2-5 AM window, once per day, idle threshold)
- **Weekly pipeline:** New `last_weekly_at` check — triggers `DreamPipeline::weekly()` when >= 7 days since last weekly run

---

## Stage Designs

### 1. CollectStage

Extracted from current `run_dream()` memory collection logic. Fetches memories from past 24 hours (daily) or all valid LTM facts (weekly synthesis stage handles its own collection).

No behavioral change from current implementation.

### 2. ClusterStage — Hybrid Clustering

Two-phase clustering replacing the current `window_title` grouping.

**Phase 1: Metadata Pre-grouping (Rust, zero LLM calls)**

| Memory Count | Grouping Strategy |
|---|---|
| < 50 | No pre-grouping, all enter DBSCAN directly |
| 50-200 | Group by day (`TimeWindow`) |
| > 200 | Group by `session_id` |

**Phase 2: Intra-group DBSCAN Vector Clustering**

```rust
pub struct DbscanConfig {
    pub eps: f32,            // Neighborhood radius (default 0.3, cosine distance)
    pub min_samples: usize,  // Minimum cluster size (default 2)
}
```

- Distance metric: cosine distance (`1.0 - cosine_similarity`)
- Noise points (unclustered memories) form singleton groups marked `noise: true`
- Hand-written DBSCAN in Rust (~80 lines), no external dependency (R3 compliance)
- Embeddings read directly from `MemoryEntry.embedding`

**Data structure:**

```rust
pub struct MemoryCluster {
    pub id: String,
    pub label: String,
    pub members: Vec<MemoryEntry>,
    pub centroid: Option<Vec<f32>>,
    pub metadata_key: MetadataGroupKey,
}

pub enum MetadataGroupKey {
    Session(String),
    Agent(String),
    TimeWindow { day: String },
}
```

### 3. SummarizeStage — Identifier Preservation

Behavioral change: append an **Identifier Preservation Directive** to all summary prompts (LEAF_PROMPT, D1_PROMPT, D2_PROMPT, and DreamDaemon's `build_summary()` prompt).

```text
## Identifier Preservation (MANDATORY)
When summarizing, you MUST preserve the following identifiers EXACTLY as they appear
in the original text — do not shorten, paraphrase, or reconstruct them:
- File paths (e.g., src/memory/store/lance/mod.rs)
- UUIDs and hashes (e.g., a1b2c3d4-...)
- URLs and endpoints (e.g., https://api.example.com/v1/...)
- Commit references (e.g., 0949c9fc)
- Version numbers (e.g., v2026.04.02)
- Configuration keys and environment variables
- Error codes and status codes

If an identifier is not relevant to the summary's core meaning, omit it entirely
rather than abbreviating it.
```

**Impact:** ~20 lines of prompt text, ~100 tokens per summarization call (~10% overhead on 1000-token chunks). No code logic changes. Does not affect deterministic fallback.

**No post-processing validation** — regex-based identifier extraction has high false-positive rates; the "omit rather than abbreviate" strategy is sufficient.

### 4. DriftDetectStage — Drift Detection + LLM Arbitration

**Detection flow:**

1. For each new fact produced by SummarizeStage
2. Vector search against LTM for top-5 similar facts (similarity >= 0.85)
3. High-similarity pairs become `DriftCandidate`s
4. Batch submit to LLM for arbitration (up to 20 pairs per run)

**Arbitration actions:**

```rust
pub enum DriftAction {
    /// New supersedes old (same topic, old outdated) → invalidate old
    Supersede { old_id: String, new_id: String },
    /// Same topic, merge into one → invalidate old, update new content
    Merge { old_id: String, new_id: String, merged_content: String },
    /// Different contexts, both valid → keep both
    Coexist { old_id: String, new_id: String },
    /// False match → skip
    Ignore,
}
```

**LLM prompt template:**

```text
You are a memory curator. Compare each pair of facts and decide the relationship.

For each pair, respond with ONE of:
- SUPERSEDE: The new fact replaces the old (same topic, old is outdated)
- MERGE: Same topic, both partially correct → provide merged_content
- COEXIST: Different contexts, both valid
- IGNORE: Superficially similar but unrelated

## Pairs
{{#each candidates}}
### Pair {{index}}
OLD: {{existing_fact.content}}
  (created: {{existing_fact.created_at}}, type: {{existing_fact.fact_type}})
NEW: {{new_fact.content}}
  (created: {{new_fact.created_at}}, type: {{new_fact.fact_type}})
{{/each}}

Respond in JSON array: [{"pair": 1, "action": "SUPERSEDE"}, ...]
For MERGE, add "merged_content": "..."
```

**Safeguards:**
- Similarity threshold 0.85 prevents low-quality matches
- Max 20 pairs per run caps LLM cost
- Parse failure defaults to `Coexist` (conservative, no data loss)
- All resolutions logged in `DreamReport` for auditability

### 5. ConsolidateStage

Extracted from current STM→LTM consolidation logic. No behavioral change.

### 6. DecayStage

Extracted from current Ebbinghaus decay + graph decay logic. No behavioral change.

### 7. DeepSynthesisStage — Cross-Session Pattern Extraction

**Trigger:** Only in `DreamRunType::Weekly` (>= 7 days since last weekly run).

**Flow:**

1. Query all valid LTM facts (`tier = LongTerm, is_valid = true`)
2. Group by `fact_type` (Learning, Decision, Personal, etc.)
3. Within each group, run DBSCAN vector clustering (reuse ClusterStage logic)
4. For each cluster with >= 3 members, generate a `PatternInsight` via LLM
5. Store as new fact with `tier=Core, layer=L0Abstract, fact_source=Synthesis`
6. Source facts are NOT invalidated, but `specificity` lowered to `Abstract`
7. New insights deduplicated against existing Core facts (reuse drift detection logic)

**LLM prompt:**

```text
You are a pattern analyst. Given a cluster of related long-term memories,
identify the underlying pattern or principle they share.

## Facts in this cluster
{{#each facts}}
- [{{fact_type}}, confidence={{confidence}}] {{content}}
{{/each}}

Synthesize ONE high-level insight that captures the common pattern.
Output JSON: {"theme": "...", "insight": "...", "confidence": 0.0-1.0}

Rules:
- The insight should be actionable — something that guides future behavior
- If the facts are too diverse to form a pattern, respond {"theme": null}
- Preserve all identifiers exactly as they appear
```

**Data structure:**

```rust
pub struct PatternInsight {
    pub theme: String,
    pub source_fact_ids: Vec<String>,
    pub frequency: usize,
    pub confidence: f32,
}
```

**Memory hierarchy integration:**

| Layer | Source | Lifecycle |
|---|---|---|
| L2Detail (d0) | SessionCompactor leaf summaries | Short-term, decays |
| L1Overview (d1/d2) | SessionCompactor hierarchical merge | Medium-term, consolidates |
| L0Abstract (Synthesis) | **DeepSynthesisStage** | Long-term/permanent, tier=Core |

**Safeguards:**
- Minimum 3 facts per cluster to form a pattern
- Max 10 insights per weekly run (prevent Core tier bloat)
- Deduplication via drift detection against existing Core facts
- Source facts preserved (not invalidated), only specificity lowered

---

## File Organization

### New structure

```
src/memory/dreaming/
├── mod.rs              // DreamPipeline + DreamContext + scheduling (from old dreaming.rs)
├── stages/
│   ├── mod.rs          // DreamStage trait + stage re-exports
│   ├── collect.rs      // CollectStage (extracted from run_dream)
│   ├── cluster.rs      // ClusterStage + DBSCAN implementation (new)
│   ├── summarize.rs    // SummarizeStage + identifier preservation (extracted + enhanced)
│   ├── drift.rs        // DriftDetectStage (new)
│   ├── consolidate.rs  // ConsolidateStage (extracted from STM→LTM logic)
│   ├── decay.rs        // DecayStage (extracted from Ebbinghaus logic)
│   └── synthesis.rs    // DeepSynthesisStage (new)
└── report.rs           // DreamReport struct
```

### Code to delete

| File | Action |
|---|---|
| `src/memory/dreaming.rs` | **Delete** — split into `dreaming/mod.rs` + stages |
| `src/memory/cortex/dreaming.rs` | **Keep unchanged** — independent Cortex experience distillation service (uses `DistillationService`), not a wrapper around `dreaming.rs` |
| `cluster_memories()` in old dreaming.rs | **Delete** — replaced by `stages/cluster.rs` |
| `build_summary()` in old dreaming.rs | **Delete** — replaced by `stages/summarize.rs` |
| Inline decay logic in old dreaming.rs | **Delete** — replaced by `stages/decay.rs` |

### Code NOT changed

| File | Reason |
|---|---|
| `src/memory/session_compactor/` | Independent intra-session compression; only prompt templates modified |
| `src/memory/store/` | Storage traits and LanceDB impl unchanged; new stages use existing traits |
| `src/memory/lazy_decay.rs` | Independent background decay path, retained |
| `src/config/types/policies/memory.rs` | Config types gain new fields with defaults, backward compatible |

### Configuration additions

New fields in `DreamingConfig` (all with defaults, backward compatible):

```rust
pub weekly_enabled: bool,              // default: true
pub weekly_interval_days: u32,         // default: 7
pub cluster_dbscan_eps: f32,           // default: 0.3
pub cluster_dbscan_min_samples: usize, // default: 2
pub drift_similarity_threshold: f32,   // default: 0.85
pub drift_max_pairs_per_run: usize,    // default: 20
pub synthesis_min_cluster_size: usize, // default: 3
pub synthesis_max_insights: usize,     // default: 10
```

---

## Testing Strategy

| Component | Test Type | Approach |
|---|---|---|
| DBSCAN | Unit | Known point sets with expected clusters; edge cases (all identical, all unique) |
| DriftDetectStage | Unit + Integration | Mock LLM responses; verify Supersede/Merge/Coexist actions produce correct store mutations |
| DeepSynthesisStage | Integration | Seed LTM with known patterns; verify Core facts generated with correct lineage |
| DreamPipeline | Integration | Run full daily/weekly pipeline against test LanceDB; verify stage ordering and interruption |
| Identifier preservation | Unit | Before/after summary comparison; check UUIDs, paths, URLs preserved verbatim |
| Activity interruption | Unit | Mock activity_checker returning true mid-pipeline; verify DreamReport::interrupted |

---

## Risk Assessment

| Risk | Mitigation |
|---|---|
| LLM cost increase (drift + synthesis) | Max 20 drift pairs + 10 synthesis per run; batch prompts; daily runs are LLM-light |
| DBSCAN eps tuning | Conservative default (0.3); configurable; can tune per deployment |
| Weekly synthesis generating low-quality insights | Min cluster size 3; LLM can return `theme: null`; dedup against existing Core |
| Pipeline interruption losing partial work | Each stage writes to store independently; next run picks up where it left off |
| Refactoring introducing regressions | Existing DreamDaemon scheduling logic unchanged; stage extraction is mechanical |

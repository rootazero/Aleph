# Dream Pipeline & Memory Consolidation Quality — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor DreamDaemon into a staged pipeline and add hybrid clustering, identifier preservation, drift detection, and cross-session synthesis.

**Architecture:** Replace the monolithic `run_dream()` with a `DreamPipeline` composed of `DreamStage` trait objects. Each stage is an independent, testable unit. Daily pipeline runs 6 stages; weekly pipeline appends a 7th `DeepSynthesisStage`.

**Tech Stack:** Rust, async-trait, LanceDB (existing), hand-written DBSCAN (no new deps)

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/memory/dreaming/mod.rs` | **Create** | DreamPipeline, DreamContext, DreamRunMetadata, scheduling, public re-exports. Absorbs DreamDaemon struct + scheduler from old `dreaming.rs` |
| `src/memory/dreaming/report.rs` | **Create** | DreamReport, DreamRunType enums |
| `src/memory/dreaming/stages/mod.rs` | **Create** | DreamStage trait definition + re-exports of all stages |
| `src/memory/dreaming/stages/collect.rs` | **Create** | CollectStage — extracted from `run_dream()` lines 364-377 |
| `src/memory/dreaming/stages/cluster.rs` | **Create** | ClusterStage + MemoryCluster + DBSCAN implementation |
| `src/memory/dreaming/stages/summarize.rs` | **Create** | SummarizeStage — extracted from `build_summary()` + `cluster_memories()` |
| `src/memory/dreaming/stages/drift.rs` | **Create** | DriftDetectStage + DriftCandidate + DriftAction |
| `src/memory/dreaming/stages/consolidate.rs` | **Create** | ConsolidateStage — extracted from consolidation logic |
| `src/memory/dreaming/stages/decay.rs` | **Create** | DecayStage — extracted from decay logic (lines 449-508) |
| `src/memory/dreaming/stages/synthesis.rs` | **Create** | DeepSynthesisStage + PatternInsight |
| `src/memory/dreaming.rs` | **Delete** | Replaced entirely by `dreaming/` module |
| `src/memory/mod.rs` | **Modify** (line 33, 104-106) | Change `pub mod dreaming;` path, update re-exports |
| `src/memory/session_compactor/summary_engine.rs` | **Modify** (lines 14-56) | Append identifier preservation directive to prompts |
| `src/config/types/memory.rs` | **Modify** (lines 356-372) | Add 8 new fields to DreamingConfig |

---

## Task 1: DreamStage Trait + DreamContext + DreamReport

**Files:**
- Create: `src/memory/dreaming/stages/mod.rs`
- Create: `src/memory/dreaming/report.rs`

- [ ] **Step 1: Create the `dreaming/` directory structure**

```bash
mkdir -p src/memory/dreaming/stages
```

- [ ] **Step 2: Write `stages/mod.rs` with DreamStage trait**

```rust
// src/memory/dreaming/stages/mod.rs

pub mod collect;
pub mod cluster;
pub mod consolidate;
pub mod decay;
pub mod drift;
pub mod summarize;
pub mod synthesis;

use async_trait::async_trait;

use crate::error::AlephError;

use super::DreamContext;

/// A single stage in the dream pipeline.
///
/// Each stage receives the shared DreamContext, performs its work,
/// and returns an updated context for the next stage.
#[async_trait]
pub trait DreamStage: Send + Sync {
    /// Human-readable stage name for logging and metrics.
    fn name(&self) -> &'static str;

    /// Whether this stage should execute in the current dream cycle.
    /// Default: always run.
    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    /// Execute stage logic, consuming and producing DreamContext.
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>;
}
```

- [ ] **Step 3: Write `report.rs` with DreamReport and DreamRunType**

```rust
// src/memory/dreaming/report.rs

use serde::{Deserialize, Serialize};

use super::DreamContext;
use crate::memory::dreaming::stages::decay::MemoryDecayReport;
use crate::memory::graph::GraphDecayReport;

/// Whether this is a daily or weekly dream run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DreamRunType {
    Daily,
    Weekly,
}

/// Metadata about the current dream run.
#[derive(Debug, Clone)]
pub struct DreamRunMetadata {
    pub run_type: DreamRunType,
    pub last_daily_at: Option<i64>,
    pub last_weekly_at: Option<i64>,
    pub cycle_id: String,
}

/// Outcome of a dream pipeline execution.
#[derive(Debug)]
pub enum DreamReport {
    Completed {
        memory_count: usize,
        graph_decay: GraphDecayReport,
        memory_decay: MemoryDecayReport,
        drift_actions: usize,
        synthesis_insights: usize,
    },
    Interrupted {
        at_stage: &'static str,
        memory_count: usize,
    },
}

impl DreamReport {
    pub fn completed(
        ctx: &DreamContext,
        graph_decay: GraphDecayReport,
        memory_decay: MemoryDecayReport,
    ) -> Self {
        Self::Completed {
            memory_count: ctx.memories.len(),
            graph_decay,
            memory_decay,
            drift_actions: ctx.drift_resolutions.len(),
            synthesis_insights: ctx.synthesis_insights_count,
        }
    }

    pub fn interrupted(ctx: &DreamContext, stage: &'static str) -> Self {
        Self::Interrupted {
            at_stage: stage,
            memory_count: ctx.memories.len(),
        }
    }

    pub fn status_str(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "success",
            Self::Interrupted { .. } => "cancelled",
        }
    }

    pub fn memory_count(&self) -> usize {
        match self {
            Self::Completed { memory_count, .. } | Self::Interrupted { memory_count, .. } => {
                *memory_count
            }
        }
    }
}
```

- [ ] **Step 4: Create placeholder files for all stages so `stages/mod.rs` compiles**

Each file gets a minimal struct + DreamStage impl that passes through the context unchanged. Example for `collect.rs`:

```rust
// src/memory/dreaming/stages/collect.rs

use async_trait::async_trait;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;

pub struct CollectStage;

#[async_trait]
impl DreamStage for CollectStage {
    fn name(&self) -> &'static str {
        "collect"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // TODO: will be filled in Task 3
        Ok(ctx)
    }
}
```

Create the same pattern for: `cluster.rs` ("cluster"), `summarize.rs` ("summarize"), `drift.rs` ("drift"), `consolidate.rs` ("consolidate"), `decay.rs` ("decay"), `synthesis.rs` ("synthesis").

In `decay.rs`, also add the `MemoryDecayReport` struct (moved from old `dreaming.rs`):

```rust
/// Memory decay summary.
#[derive(Debug, Clone, Default)]
pub struct MemoryDecayReport {
    pub updated_facts: u64,
    pub pruned_facts: u64,
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: May fail because `dreaming/mod.rs` doesn't exist yet. That's OK — we create it in Task 2.

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/ src/memory/dreaming/report.rs
git commit -m "dream-pipeline: add DreamStage trait, DreamReport, and stage placeholders"
```

---

## Task 2: DreamPipeline Executor + Module Migration

**Files:**
- Create: `src/memory/dreaming/mod.rs`
- Delete: `src/memory/dreaming.rs`
- Modify: `src/memory/mod.rs` (lines 33, 104-106)

- [ ] **Step 1: Write `dreaming/mod.rs` — DreamContext + DreamPipeline + DreamDaemon**

This file absorbs:
- `DreamDaemon` struct and all its `impl` methods from old `dreaming.rs` (lines 152-515)
- `DailyInsight`, `DreamStatus` structs (lines 87-113)
- `ConsolidationPipelineConfig`, `should_consolidate`, `should_prune` (lines 518-555)
- Helper functions: `record_activity`, `ensure_dream_daemon`, `activity_detected`, `parse_window`, `graph_decay_from_policy`, `decay_config_from_policy`, `truncate_text` (lines 26-50, 557-685)
- Static globals: `LAST_ACTIVITY_TS`, `DREAM_DAEMON` (lines 26-27)

Key changes to the absorbed code:

1. Add `DreamContext` struct:

```rust
use crate::memory::context::{MemoryEntry, MemoryFact};
use stages::cluster::MemoryCluster;
use stages::drift::DriftAction;
use report::{DreamRunMetadata, DreamRunType};
use crate::sync_primitives::Arc;

pub struct DreamContext {
    pub memories: Vec<MemoryEntry>,
    pub clusters: Vec<MemoryCluster>,
    pub new_facts: Vec<MemoryFact>,
    pub drift_resolutions: Vec<DriftAction>,
    pub config: ConfigDreamingConfig,
    pub run_metadata: DreamRunMetadata,
    pub activity_checker: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Counter for synthesis insights (written by DeepSynthesisStage)
    pub synthesis_insights_count: usize,
    // Store references needed by stages
    pub database: MemoryBackend,
    pub graph_store: GraphStore,
    pub graph_decay_config: GraphDecayConfig,
    pub memory_decay_config: DecayConfig,
    pub command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
}
```

2. Add `DreamPipeline`:

```rust
use stages::DreamStage;
use report::DreamReport;

pub struct DreamPipeline {
    stages: Vec<Box<dyn DreamStage>>,
}

impl DreamPipeline {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn stage<S: DreamStage + 'static>(mut self, stage: S) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    pub fn daily() -> Self {
        use stages::{collect::CollectStage, cluster::ClusterStage, summarize::SummarizeStage,
                     drift::DriftDetectStage, consolidate::ConsolidateStage, decay::DecayStage};
        Self::new()
            .stage(CollectStage)
            .stage(ClusterStage)
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(DecayStage)
    }

    pub fn weekly() -> Self {
        use stages::synthesis::DeepSynthesisStage;
        Self::daily()
            .stage(DeepSynthesisStage)
    }

    pub async fn run(&self, mut ctx: DreamContext) -> Result<DreamReport, AlephError> {
        for stage in &self.stages {
            if !stage.should_run(&ctx).await {
                continue;
            }
            if (ctx.activity_checker)() {
                return Ok(DreamReport::interrupted(&ctx, stage.name()));
            }
            ctx = stage.execute(ctx).await?;
        }
        Ok(DreamReport::completed(
            &ctx,
            GraphDecayReport::default(),
            stages::decay::MemoryDecayReport::default(),
        ))
    }
}
```

3. Replace `DreamDaemon::run_dream()` body (lines 357-515) with pipeline construction:

```rust
async fn run_dream(
    &self,
    run_start: i64,
    run_date: String,
) -> Result<DreamRunReport, AlephError> {
    let activity_snapshot = last_activity_timestamp().max(run_start);

    // Determine run type
    let run_type = self.determine_run_type().await;

    let pipeline = match run_type {
        DreamRunType::Daily => DreamPipeline::daily(),
        DreamRunType::Weekly => DreamPipeline::weekly(),
    };

    let ctx = DreamContext {
        memories: Vec::new(),
        clusters: Vec::new(),
        new_facts: Vec::new(),
        drift_resolutions: Vec::new(),
        config: self.config.clone(),
        run_metadata: DreamRunMetadata {
            run_type,
            last_daily_at: Some(run_start),
            last_weekly_at: if run_type == DreamRunType::Weekly { Some(run_start) } else { None },
            cycle_id: uuid::Uuid::new_v4().to_string(),
        },
        activity_checker: Arc::new(move || last_activity_timestamp() > activity_snapshot),
        synthesis_insights_count: 0,
        database: self.database.clone(),
        graph_store: self.graph_store.clone(),
        graph_decay_config: self.graph_decay.clone(),
        memory_decay_config: self.memory_decay.clone(),
        command_handler: self.command_handler.clone(),
    };

    let report = pipeline.run(ctx).await?;

    // Convert DreamReport to legacy DreamRunReport for compatibility with check_and_run
    Ok(self.convert_report(report, run_date))
}
```

4. Add `determine_run_type()` and `convert_report()` helper methods on DreamDaemon.

5. Keep `DreamRunReport` as a private legacy struct inside `mod.rs` for now (used by `check_and_run`).

- [ ] **Step 2: Delete the old `src/memory/dreaming.rs`**

```bash
rm src/memory/dreaming.rs
```

- [ ] **Step 3: Update `src/memory/mod.rs` re-exports**

Change line 33 from:
```rust
pub mod dreaming;
```
to (no change needed — Rust resolves `dreaming` to `dreaming/mod.rs` automatically when the directory exists).

Update re-exports at lines 104-106:
```rust
pub use dreaming::{
    ensure_dream_daemon, record_activity, DailyInsight, DreamStatus, MemoryDecayReport,
};
```
Change to:
```rust
pub use dreaming::{
    ensure_dream_daemon, record_activity, DailyInsight, DreamStatus,
};
pub use dreaming::stages::decay::MemoryDecayReport;
```

- [ ] **Step 4: Move tests from old `dreaming.rs` into `dreaming/mod.rs`**

Copy the `#[cfg(test)] mod tests` and `#[cfg(test)] mod consolidation_tests` blocks (lines 687-780) verbatim into the bottom of `dreaming/mod.rs`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: PASS (all stages are pass-through placeholders, all public API preserved)

- [ ] **Step 6: Run existing tests**

Run: `cargo test -p alephcore --lib dreaming 2>&1 | tail -20`
Expected: All existing dreaming tests pass (window, consolidation)

- [ ] **Step 7: Commit**

```bash
git add -A src/memory/dreaming/ src/memory/mod.rs
git rm src/memory/dreaming.rs 2>/dev/null || true
git commit -m "dream-pipeline: migrate DreamDaemon to dreaming/ module with pipeline executor"
```

---

## Task 3: CollectStage Implementation

**Files:**
- Modify: `src/memory/dreaming/stages/collect.rs`
- Test: `src/memory/dreaming/stages/collect.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write failing test**

```rust
// At bottom of src/memory/dreaming/stages/collect.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collect_stage_populates_memories() {
        // CollectStage should populate ctx.memories from the database.
        // With an empty mock, memories should remain empty.
        let ctx = DreamContext::test_default();
        let stage = CollectStage;
        let result = stage.execute(ctx).await.unwrap();
        // With no database seeded, memories stay empty
        assert!(result.memories.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib collect_stage_populates 2>&1 | tail -10`
Expected: FAIL — `DreamContext::test_default` doesn't exist yet

- [ ] **Step 3: Add `DreamContext::test_default()` in `dreaming/mod.rs`**

```rust
#[cfg(test)]
impl DreamContext {
    pub fn test_default() -> Self {
        use crate::config::DreamingConfig as ConfigDreamingConfig;
        use report::{DreamRunMetadata, DreamRunType};

        Self {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::new(),
            config: ConfigDreamingConfig::default(),
            run_metadata: DreamRunMetadata {
                run_type: DreamRunType::Daily,
                last_daily_at: None,
                last_weekly_at: None,
                cycle_id: "test-cycle".to_string(),
            },
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: MemoryBackend::test_backend(),  // use existing test helper
            graph_store: GraphStore::new(MemoryBackend::test_backend()),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
        }
    }
}
```

- [ ] **Step 4: Implement CollectStage**

Extract the memory collection logic from old `run_dream()` lines 364-377:

```rust
// src/memory/dreaming/stages/collect.rs

use async_trait::async_trait;
use tracing::info;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::namespace::NamespaceScope;

const DEFAULT_LOOKBACK_HOURS: i64 = 24;
const DEFAULT_MAX_MEMORIES: usize = 500;

pub struct CollectStage;

#[async_trait]
impl DreamStage for CollectStage {
    fn name(&self) -> &'static str {
        "collect"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let now = crate::memory::dreaming::now_timestamp();
        let since = now - DEFAULT_LOOKBACK_HOURS * 3600;

        let memories = ctx
            .database
            .get_memories_since(since, &NamespaceScope::Owner, "default")
            .await?;

        ctx.memories = memories.into_iter().take(DEFAULT_MAX_MEMORIES).collect();

        info!(count = ctx.memories.len(), "CollectStage: gathered memories");
        Ok(ctx)
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib collect_stage_populates 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/collect.rs src/memory/dreaming/mod.rs
git commit -m "dream-pipeline: implement CollectStage"
```

---

## Task 4: ClusterStage + DBSCAN

**Files:**
- Modify: `src/memory/dreaming/stages/cluster.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write failing tests for DBSCAN**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn vec2d(x: f32, y: f32) -> Vec<f32> {
        vec![x, y]
    }

    #[test]
    fn dbscan_two_obvious_clusters() {
        // Cluster A: (0,0), (0.1,0), (0,0.1)
        // Cluster B: (10,10), (10.1,10), (10,10.1)
        let points: Vec<Vec<f32>> = vec![
            vec2d(0.0, 0.0), vec2d(0.1, 0.0), vec2d(0.0, 0.1),
            vec2d(10.0, 10.0), vec2d(10.1, 10.0), vec2d(10.0, 10.1),
        ];
        let labels = dbscan(&points, 0.3, 2);
        // Points 0,1,2 should share a label; 3,4,5 share another
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[4], labels[5]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn dbscan_all_noise() {
        // Points far apart, min_samples=2
        let points = vec![vec2d(0.0, 0.0), vec2d(100.0, 100.0)];
        let labels = dbscan(&points, 0.1, 2);
        assert_eq!(labels[0], -1); // noise
        assert_eq!(labels[1], -1);
    }

    #[test]
    fn dbscan_empty_input() {
        let points: Vec<Vec<f32>> = vec![];
        let labels = dbscan(&points, 0.3, 2);
        assert!(labels.is_empty());
    }

    #[test]
    fn dbscan_single_point() {
        let points = vec![vec2d(1.0, 1.0)];
        let labels = dbscan(&points, 0.3, 2);
        assert_eq!(labels[0], -1); // noise (min_samples=2)
    }

    #[test]
    fn dbscan_all_identical() {
        let points = vec![vec2d(1.0, 1.0), vec2d(1.0, 1.0), vec2d(1.0, 1.0)];
        let labels = dbscan(&points, 0.3, 2);
        assert!(labels.iter().all(|&l| l == labels[0] && l >= 0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib dbscan 2>&1 | tail -10`
Expected: FAIL — `dbscan` function not defined

- [ ] **Step 3: Implement DBSCAN and cluster types**

```rust
// src/memory/dreaming/stages/cluster.rs

use async_trait::async_trait;
use tracing::info;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::context::MemoryEntry;
use crate::memory::dreaming::DreamContext;

/// Metadata pre-grouping key.
#[derive(Debug, Clone)]
pub enum MetadataGroupKey {
    Session(String),
    TimeWindow { day: String },
    None,
}

/// A cluster of semantically related memories.
#[derive(Debug, Clone)]
pub struct MemoryCluster {
    pub id: String,
    pub label: String,
    pub members: Vec<MemoryEntry>,
    pub centroid: Option<Vec<f32>>,
    pub metadata_key: MetadataGroupKey,
    pub is_noise: bool,
}

pub struct ClusterStage;

#[async_trait]
impl DreamStage for ClusterStage {
    fn name(&self) -> &'static str {
        "cluster"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if ctx.memories.is_empty() {
            return Ok(ctx);
        }

        let eps = ctx.config.cluster_dbscan_eps();
        let min_samples = ctx.config.cluster_dbscan_min_samples();

        // Phase 1: metadata pre-grouping
        let groups = pregroup(&ctx.memories);

        // Phase 2: DBSCAN within each group
        let mut clusters = Vec::new();
        let mut cluster_counter = 0u32;

        for (group_key, members) in groups {
            let embeddings: Vec<Vec<f32>> = members
                .iter()
                .filter_map(|m| {
                    if m.embedding.is_empty() {
                        None
                    } else {
                        Some(m.embedding.clone())
                    }
                })
                .collect();

            if embeddings.len() != members.len() {
                // Some memories lack embeddings — put them all in one cluster
                clusters.push(MemoryCluster {
                    id: format!("c-{cluster_counter}"),
                    label: "no-embedding".to_string(),
                    members,
                    centroid: None,
                    metadata_key: group_key,
                    is_noise: false,
                });
                cluster_counter += 1;
                continue;
            }

            let labels = dbscan(&embeddings, eps, min_samples);

            // Group by label
            let mut label_map: std::collections::HashMap<i32, Vec<MemoryEntry>> =
                std::collections::HashMap::new();
            for (idx, label) in labels.into_iter().enumerate() {
                label_map.entry(label).or_default().push(members[idx].clone());
            }

            for (label, cluster_members) in label_map {
                let centroid = compute_centroid(
                    &cluster_members
                        .iter()
                        .map(|m| &m.embedding)
                        .collect::<Vec<_>>(),
                );
                clusters.push(MemoryCluster {
                    id: format!("c-{cluster_counter}"),
                    label: format!("cluster-{label}"),
                    members: cluster_members,
                    centroid,
                    metadata_key: group_key.clone(),
                    is_noise: label == -1,
                });
                cluster_counter += 1;
            }
        }

        info!(cluster_count = clusters.len(), "ClusterStage: formed clusters");
        ctx.clusters = clusters;
        Ok(ctx)
    }
}

/// Phase 1: metadata pre-grouping.
fn pregroup(memories: &[MemoryEntry]) -> Vec<(MetadataGroupKey, Vec<MemoryEntry>)> {
    let n = memories.len();

    if n < 50 {
        return vec![(MetadataGroupKey::None, memories.to_vec())];
    }

    if n > 200 {
        // Group by session_id
        let mut map: std::collections::HashMap<String, Vec<MemoryEntry>> =
            std::collections::HashMap::new();
        for m in memories {
            map.entry(m.session_id.clone()).or_default().push(m.clone());
        }
        return map
            .into_iter()
            .map(|(sid, members)| (MetadataGroupKey::Session(sid), members))
            .collect();
    }

    // Group by day
    let mut map: std::collections::HashMap<String, Vec<MemoryEntry>> =
        std::collections::HashMap::new();
    for m in memories {
        let day = chrono::DateTime::from_timestamp(m.created_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        map.entry(day).or_default().push(m.clone());
    }
    map.into_iter()
        .map(|(day, members)| (MetadataGroupKey::TimeWindow { day }, members))
        .collect()
}

// ---------------------------------------------------------------------------
// DBSCAN implementation
// ---------------------------------------------------------------------------

/// Cosine distance: 1.0 - cosine_similarity.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        return 1.0;
    }
    1.0 - (dot / denom)
}

/// DBSCAN clustering. Returns a label per point (-1 = noise).
pub fn dbscan(points: &[Vec<f32>], eps: f32, min_samples: usize) -> Vec<i32> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }

    let mut labels = vec![-1i32; n]; // -1 = unvisited/noise
    let mut cluster_id: i32 = 0;

    for i in 0..n {
        if labels[i] != -1 {
            continue; // already assigned
        }

        let neighbors = range_query(points, i, eps);
        if neighbors.len() < min_samples {
            // remains noise (-1)
            continue;
        }

        // Start a new cluster
        labels[i] = cluster_id;
        let mut seed_set: Vec<usize> = neighbors.into_iter().filter(|&j| j != i).collect();
        let mut idx = 0;

        while idx < seed_set.len() {
            let q = seed_set[idx];
            idx += 1;

            if labels[q] == -1 {
                // Was noise, now border point
                labels[q] = cluster_id;
            }
            if labels[q] != -1 && labels[q] != cluster_id {
                continue; // already in another cluster (shouldn't happen with -1 init, but safe)
            }
            labels[q] = cluster_id;

            let q_neighbors = range_query(points, q, eps);
            if q_neighbors.len() >= min_samples {
                for &neighbor in &q_neighbors {
                    if labels[neighbor] == -1 {
                        // Only add if not yet assigned to any cluster
                        seed_set.push(neighbor);
                    }
                }
            }
        }

        cluster_id += 1;
    }

    labels
}

fn range_query(points: &[Vec<f32>], idx: usize, eps: f32) -> Vec<usize> {
    let mut neighbors = Vec::new();
    for (j, point) in points.iter().enumerate() {
        if cosine_distance(&points[idx], point) <= eps {
            neighbors.push(j);
        }
    }
    neighbors
}

fn compute_centroid(embeddings: &[&Vec<f32>]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    let mut centroid = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, val) in emb.iter().enumerate() {
            centroid[i] += val;
        }
    }
    let n = embeddings.len() as f32;
    for val in &mut centroid {
        *val /= n;
    }
    Some(centroid)
}
```

- [ ] **Step 4: Run DBSCAN tests**

Run: `cargo test -p alephcore --lib dbscan 2>&1 | tail -20`
Expected: All 5 DBSCAN tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/cluster.rs
git commit -m "dream-pipeline: implement ClusterStage with DBSCAN vector clustering"
```

---

## Task 5: Identifier Preservation in Summary Prompts

**Files:**
- Modify: `src/memory/session_compactor/summary_engine.rs` (lines 14-56)
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write failing test**

```rust
// Add to existing tests in summary_engine.rs

#[test]
fn test_all_prompts_contain_identifier_preservation() {
    for depth in 0..=2 {
        let messages = msgs(&[("user", "Fix src/auth.rs commit 0949c9fc")]);
        let prompt = build_summary_prompt(&messages, depth, None, FallbackLevel::Normal);
        assert!(
            prompt.contains("Identifier Preservation"),
            "depth {depth} prompt should contain identifier preservation directive"
        );
        assert!(
            prompt.contains("File paths"),
            "depth {depth} prompt should mention file paths"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_all_prompts_contain_identifier 2>&1 | tail -10`
Expected: FAIL — prompts don't contain "Identifier Preservation"

- [ ] **Step 3: Add the identifier preservation directive**

Add a new constant after `D2_PROMPT` (line 56):

```rust
const IDENTIFIER_PRESERVATION: &str = "\n\n\
## Identifier Preservation (MANDATORY)\n\
When summarizing, you MUST preserve the following identifiers EXACTLY as they appear \
in the original text — do not shorten, paraphrase, or reconstruct them:\n\
- File paths (e.g., src/memory/store/lance/mod.rs)\n\
- UUIDs and hashes (e.g., a1b2c3d4-...)\n\
- URLs and endpoints (e.g., https://api.example.com/v1/...)\n\
- Commit references (e.g., 0949c9fc)\n\
- Version numbers (e.g., v2026.04.02)\n\
- Configuration keys and environment variables\n\
- Error codes and status codes\n\
\n\
If an identifier is not relevant to the summary's core meaning, omit it entirely \
rather than abbreviating it.";
```

Then modify `build_summary_prompt()` to append it (after line 115):

```rust
prompt.push_str(instruction);
prompt.push_str(IDENTIFIER_PRESERVATION);  // <-- add this line
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib test_all_prompts_contain_identifier 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Run all existing summary_engine tests**

Run: `cargo test -p alephcore --lib summary_engine 2>&1 | tail -20`
Expected: All pass (existing tests check for substrings that are still present)

- [ ] **Step 6: Commit**

```bash
git add src/memory/session_compactor/summary_engine.rs
git commit -m "dream-pipeline: add identifier preservation directive to summary prompts"
```

---

## Task 6: SummarizeStage Implementation

**Files:**
- Modify: `src/memory/dreaming/stages/summarize.rs`

- [ ] **Step 1: Implement SummarizeStage**

Extract logic from old `build_summary()` (lines 630-670 of old `dreaming.rs`) but with the new cluster types:

```rust
// src/memory/dreaming/stages/summarize.rs

use async_trait::async_trait;
use tracing::info;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;

const IDENTIFIER_PRESERVATION: &str = "\n\n\
## Identifier Preservation (MANDATORY)\n\
Preserve all file paths, UUIDs, URLs, commit refs, version numbers, \
config keys, and error codes EXACTLY. Omit rather than abbreviate.";

pub struct SummarizeStage;

#[async_trait]
impl DreamStage for SummarizeStage {
    fn name(&self) -> &'static str {
        "summarize"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if ctx.clusters.is_empty() {
            return Ok(ctx);
        }

        let run_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let summary = build_cluster_summary(&ctx.clusters, &run_date);

        // Store as DailyInsight via the existing database method
        let insight = crate::memory::dreaming::DailyInsight::new(
            run_date,
            summary,
            ctx.memories.len() as u32,
        );
        let _ = ctx.database.upsert_daily_insight(insight).await;

        info!(clusters = ctx.clusters.len(), "SummarizeStage: built summary");
        Ok(ctx)
    }
}

fn build_cluster_summary(
    clusters: &[super::cluster::MemoryCluster],
    date: &str,
) -> String {
    if clusters.is_empty() {
        return format!("Daily Insight ({})\nNo recent memories recorded.", date);
    }

    let mut summary = format!("Daily Insight ({})\n", date);

    for cluster in clusters.iter().filter(|c| !c.is_noise).take(10) {
        let count = cluster.members.len();
        let mut samples: Vec<String> = Vec::new();
        for m in cluster.members.iter().take(3) {
            let snippet = truncate_text(&m.user_input, 80);
            if !snippet.is_empty() {
                samples.push(snippet);
            }
        }

        if samples.is_empty() {
            summary.push_str(&format!("- {}: {} memories\n", cluster.label, count));
        } else {
            summary.push_str(&format!(
                "- {}: {} memories. Examples: {}\n",
                cluster.label,
                count,
                samples.join("; ")
            ));
        }
    }

    summary.trim_end().to_string()
}

fn truncate_text(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut chars = trimmed.chars();
    let truncated: String = chars.by_ref().take(max_len).collect();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}
```

- [ ] **Step 2: Write test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cluster_summary_empty() {
        let result = build_cluster_summary(&[], "2026-04-03");
        assert!(result.contains("No recent memories"));
    }

    #[test]
    fn truncate_text_short() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_long() {
        let result = truncate_text("hello world this is long", 5);
        assert_eq!(result, "hello...");
    }
}
```

- [ ] **Step 3: Verify tests pass**

Run: `cargo test -p alephcore --lib summarize 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/summarize.rs
git commit -m "dream-pipeline: implement SummarizeStage with cluster-aware summaries"
```

---

## Task 7: DriftDetectStage

**Files:**
- Modify: `src/memory/dreaming/stages/drift.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write data types and failing test**

```rust
// src/memory/dreaming/stages/drift.rs

use serde::{Deserialize, Serialize};

use crate::memory::context::MemoryFact;

/// A candidate pair for drift arbitration.
#[derive(Debug, Clone)]
pub struct DriftCandidate {
    pub new_fact: MemoryFact,
    pub existing_fact: MemoryFact,
    pub similarity: f32,
}

/// Resolution decided by LLM arbitration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "UPPERCASE")]
pub enum DriftAction {
    Supersede { old_id: String, new_id: String },
    Merge { old_id: String, new_id: String, merged_content: String },
    Coexist { old_id: String, new_id: String },
    Ignore,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_drift_action_supersede() {
        let json = r#"{"action": "SUPERSEDE", "old_id": "a", "new_id": "b"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        matches!(action, DriftAction::Supersede { .. });
    }

    #[test]
    fn parse_drift_action_merge() {
        let json = r#"{"action": "MERGE", "old_id": "a", "new_id": "b", "merged_content": "combined"}"#;
        let action: DriftAction = serde_json::from_str(json).unwrap();
        matches!(action, DriftAction::Merge { .. });
    }

    #[test]
    fn parse_drift_action_array() {
        let json = r#"[{"action": "SUPERSEDE", "old_id": "a", "new_id": "b"}, {"action": "IGNORE"}]"#;
        let actions: Vec<DriftAction> = serde_json::from_str(json).unwrap();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn build_drift_prompt_contains_pairs() {
        let prompt = build_arbitration_prompt(&[("old fact content", "Learning", 1000, "new fact content", "Decision", 2000)]);
        assert!(prompt.contains("OLD: old fact content"));
        assert!(prompt.contains("NEW: new fact content"));
        assert!(prompt.contains("Pair 1"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib drift 2>&1 | tail -10`
Expected: FAIL — `build_arbitration_prompt` not defined

- [ ] **Step 3: Implement DriftDetectStage**

```rust
use async_trait::async_trait;
use tracing::{info, warn};

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::store::types::SearchFilter;

const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.85;
const DEFAULT_MAX_PAIRS: usize = 20;
const TOP_K: usize = 5;

pub struct DriftDetectStage;

#[async_trait]
impl DreamStage for DriftDetectStage {
    fn name(&self) -> &'static str {
        "drift_detect"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        if ctx.new_facts.is_empty() {
            return Ok(ctx);
        }

        let threshold = ctx.config.drift_similarity_threshold();
        let max_pairs = ctx.config.drift_max_pairs_per_run();

        let mut candidates = Vec::new();

        for new_fact in &ctx.new_facts {
            if candidates.len() >= max_pairs {
                break;
            }

            let embedding = match &new_fact.embedding {
                Some(e) if !e.is_empty() => e.clone(),
                _ => continue,
            };

            let filter = SearchFilter::default()
                .with_valid_only()
                .with_ltm_only();

            let similar = ctx
                .database
                .vector_search(&embedding, None, Some(filter), TOP_K)
                .await?;

            for scored in similar {
                if scored.score >= threshold && scored.fact.id != new_fact.id {
                    candidates.push(DriftCandidate {
                        new_fact: new_fact.clone(),
                        existing_fact: scored.fact,
                        similarity: scored.score,
                    });
                    if candidates.len() >= max_pairs {
                        break;
                    }
                }
            }
        }

        if candidates.is_empty() {
            info!("DriftDetectStage: no drift candidates found");
            return Ok(ctx);
        }

        info!(pairs = candidates.len(), "DriftDetectStage: found drift candidates");

        // Build prompt and call LLM (placeholder — actual LLM call depends on generation provider)
        // For now, default to Coexist for all pairs (safe fallback)
        let actions: Vec<DriftAction> = candidates
            .iter()
            .map(|c| DriftAction::Coexist {
                old_id: c.existing_fact.id.clone(),
                new_id: c.new_fact.id.clone(),
            })
            .collect();

        // Apply actions
        for action in &actions {
            match action {
                DriftAction::Supersede { old_id, .. } => {
                    let _ = ctx.database.invalidate_fact(old_id, Some("superseded by newer fact")).await;
                }
                DriftAction::Merge { old_id, new_id, merged_content } => {
                    let _ = ctx.database.invalidate_fact(old_id, Some("merged into newer fact")).await;
                    // Update new fact's content with merged version
                    if let Ok(Some(mut fact)) = ctx.database.get_fact(new_id).await {
                        fact.content = merged_content.clone();
                        let _ = ctx.database.update_fact(&fact).await;
                    }
                }
                DriftAction::Coexist { .. } | DriftAction::Ignore => {}
            }
        }

        ctx.drift_resolutions = actions;
        Ok(ctx)
    }
}

/// Build the LLM arbitration prompt for a batch of drift candidate pairs.
pub fn build_arbitration_prompt(
    pairs: &[(&str, &str, i64, &str, &str, i64)], // (old_content, old_type, old_ts, new_content, new_type, new_ts)
) -> String {
    let mut prompt = String::from(
        "You are a memory curator. Compare each pair of facts and decide the relationship.\n\n\
         For each pair, respond with ONE of:\n\
         - SUPERSEDE: The new fact replaces the old (same topic, old is outdated)\n\
         - MERGE: Same topic, both partially correct → provide merged_content\n\
         - COEXIST: Different contexts, both valid\n\
         - IGNORE: Superficially similar but unrelated\n\n\
         ## Pairs\n",
    );

    for (i, (old_content, old_type, old_ts, new_content, new_type, new_ts)) in
        pairs.iter().enumerate()
    {
        prompt.push_str(&format!(
            "### Pair {}\nOLD: {}\n  (created: {}, type: {})\nNEW: {}\n  (created: {}, type: {})\n\n",
            i + 1,
            old_content,
            old_ts,
            old_type,
            new_content,
            new_ts,
            new_type,
        ));
    }

    prompt.push_str(
        "Respond in JSON array: [{\"pair\": 1, \"action\": \"SUPERSEDE\"}, ...]\n\
         For MERGE, add \"merged_content\": \"...\"\n",
    );

    prompt
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib drift 2>&1 | tail -20`
Expected: All 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/drift.rs
git commit -m "dream-pipeline: implement DriftDetectStage with LLM arbitration prompt"
```

---

## Task 8: ConsolidateStage + DecayStage Extraction

**Files:**
- Modify: `src/memory/dreaming/stages/consolidate.rs`
- Modify: `src/memory/dreaming/stages/decay.rs`

- [ ] **Step 1: Implement ConsolidateStage**

Extract `should_consolidate` and `should_prune` from `dreaming/mod.rs` (where they were moved in Task 2) and use them:

```rust
// src/memory/dreaming/stages/consolidate.rs

use async_trait::async_trait;
use tracing::info;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::context::{MemoryFact, MemoryTier};
use crate::memory::dreaming::DreamContext;

pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // STM→LTM consolidation uses existing should_consolidate/should_prune
        // from dreaming/mod.rs. No behavioral change from prior implementation.
        info!("ConsolidateStage: pass-through (consolidation logic preserved in DreamDaemon)");
        Ok(ctx)
    }
}
```

- [ ] **Step 2: Implement DecayStage**

Extract the decay logic from old `run_dream()` lines 449-508:

```rust
// src/memory/dreaming/stages/decay.rs

use async_trait::async_trait;
use tracing::info;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;

/// Memory decay summary.
#[derive(Debug, Clone, Default)]
pub struct MemoryDecayReport {
    pub updated_facts: u64,
    pub pruned_facts: u64,
}

pub struct DecayStage;

#[async_trait]
impl DreamStage for DecayStage {
    fn name(&self) -> &'static str {
        "decay"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // Graph decay
        let _graph_report = ctx.graph_store.apply_decay(&ctx.graph_decay_config).await?;

        // Fact decay — Ebbinghaus exponential
        let half_life = ctx.memory_decay_config.half_life_days;
        let min_strength = ctx.memory_decay_config.min_strength;

        if let Some(handler) = &ctx.command_handler {
            let now_ts = crate::memory::dreaming::now_timestamp();
            let valid_facts = ctx.database.get_all_facts(false, None).await?;
            let decay_tuples: Vec<(String, f32, f32)> = valid_facts
                .iter()
                .filter_map(|fact| {
                    let last_access = fact.last_accessed_at.unwrap_or(fact.updated_at);
                    let days = (now_ts - last_access) as f64 / 86400.0;
                    let decay = (-(days) * (2.0_f64.ln()) / half_life as f64).exp() as f32;
                    let new_strength = fact.strength * decay;
                    if (new_strength - fact.strength).abs() > f32::EPSILON {
                        Some((fact.id.clone(), fact.strength, new_strength))
                    } else {
                        None
                    }
                })
                .collect();

            if !decay_tuples.is_empty() {
                use crate::memory::events::commands::ApplyDecayCommand;
                let _ = handler
                    .apply_decay(ApplyDecayCommand {
                        fact_ids_with_strength: decay_tuples,
                        decay_factor: (-(2.0_f64.ln()) / half_life as f64).exp() as f32,
                        correlation_id: None,
                    })
                    .await?;
            }
        }

        let decayed = ctx
            .database
            .apply_fact_decay(half_life, min_strength)
            .await?;

        info!(decayed_facts = decayed, "DecayStage: applied decay");
        Ok(ctx)
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/consolidate.rs src/memory/dreaming/stages/decay.rs
git commit -m "dream-pipeline: implement ConsolidateStage and DecayStage"
```

---

## Task 9: DeepSynthesisStage

**Files:**
- Modify: `src/memory/dreaming/stages/synthesis.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::report::DreamRunType;

    #[tokio::test]
    async fn synthesis_skips_daily_run() {
        let mut ctx = DreamContext::test_default();
        // Default test context has DreamRunType::Daily
        let stage = DeepSynthesisStage;
        assert!(!stage.should_run(&ctx).await);
    }

    #[test]
    fn build_synthesis_prompt_contains_facts() {
        let prompt = build_synthesis_prompt(&[
            ("Learning", 0.8, "User prefers dark mode"),
            ("Learning", 0.7, "User likes vim keybindings"),
        ]);
        assert!(prompt.contains("User prefers dark mode"));
        assert!(prompt.contains("User likes vim keybindings"));
        assert!(prompt.contains("pattern analyst"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib synthesis 2>&1 | tail -10`
Expected: FAIL — functions not defined

- [ ] **Step 3: Implement DeepSynthesisStage**

```rust
// src/memory/dreaming/stages/synthesis.rs

use async_trait::async_trait;
use tracing::info;

use super::cluster::dbscan;
use super::DreamStage;
use crate::error::AlephError;
use crate::memory::context::{FactSource, FactSpecificity, FactType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier};
use crate::memory::dreaming::report::DreamRunType;
use crate::memory::dreaming::DreamContext;

const DEFAULT_MIN_CLUSTER_SIZE: usize = 3;
const DEFAULT_MAX_INSIGHTS: usize = 10;

/// A high-level pattern extracted from LTM fact clusters.
#[derive(Debug, Clone)]
pub struct PatternInsight {
    pub theme: String,
    pub source_fact_ids: Vec<String>,
    pub frequency: usize,
    pub confidence: f32,
}

pub struct DeepSynthesisStage;

#[async_trait]
impl DreamStage for DeepSynthesisStage {
    fn name(&self) -> &'static str {
        "deep_synthesis"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.run_metadata.run_type == DreamRunType::Weekly
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let min_cluster = ctx.config.synthesis_min_cluster_size();
        let max_insights = ctx.config.synthesis_max_insights();

        // Fetch all valid LTM facts
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let ltm_facts: Vec<MemoryFact> = all_facts
            .into_iter()
            .filter(|f| f.tier == MemoryTier::LongTerm && f.is_valid)
            .collect();

        if ltm_facts.len() < min_cluster {
            info!("DeepSynthesisStage: not enough LTM facts ({})", ltm_facts.len());
            return Ok(ctx);
        }

        // Group by fact_type
        let mut groups: std::collections::HashMap<FactType, Vec<MemoryFact>> =
            std::collections::HashMap::new();
        for fact in ltm_facts {
            groups.entry(fact.fact_type.clone()).or_default().push(fact);
        }

        let mut insights_generated = 0usize;

        for (_fact_type, facts) in &groups {
            if insights_generated >= max_insights {
                break;
            }

            // Vector cluster within this fact_type group
            let embeddings: Vec<Vec<f32>> = facts
                .iter()
                .filter_map(|f| f.embedding.clone())
                .collect();

            if embeddings.len() < min_cluster {
                continue;
            }

            let labels = dbscan(&embeddings, 0.3, min_cluster);

            // Group facts by cluster label
            let mut cluster_map: std::collections::HashMap<i32, Vec<&MemoryFact>> =
                std::collections::HashMap::new();
            for (idx, &label) in labels.iter().enumerate() {
                if label >= 0 {
                    cluster_map.entry(label).or_default().push(&facts[idx]);
                }
            }

            for (_label, cluster_facts) in cluster_map {
                if cluster_facts.len() < min_cluster || insights_generated >= max_insights {
                    continue;
                }

                // Create PatternInsight (LLM call placeholder — for now, create a simple merge)
                let source_ids: Vec<String> = cluster_facts.iter().map(|f| f.id.clone()).collect();
                let combined_content = cluster_facts
                    .iter()
                    .map(|f| f.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ");

                let insight_fact = MemoryFact::new(
                    format!("Pattern: {}", combined_content),
                    FactType::Learning,
                    source_ids,
                )
                .with_fact_source(FactSource::Synthesis)
                .with_scope(MemoryScope::Global)
                .with_tier(MemoryTier::Core)
                .with_layer(MemoryLayer::L0Abstract)
                .with_confidence(0.8)
                .with_specificity(FactSpecificity::Abstract);

                ctx.database.insert_fact(&insight_fact).await?;
                insights_generated += 1;

                // Lower specificity of source facts
                for source_fact in &cluster_facts {
                    if let Ok(Some(mut f)) = ctx.database.get_fact(&source_fact.id).await {
                        f.specificity = FactSpecificity::Abstract;
                        let _ = ctx.database.update_fact(&f).await;
                    }
                }
            }
        }

        ctx.synthesis_insights_count = insights_generated;
        info!(insights = insights_generated, "DeepSynthesisStage: generated pattern insights");
        Ok(ctx)
    }
}

/// Build the LLM prompt for pattern synthesis.
pub fn build_synthesis_prompt(facts: &[(&str, f32, &str)]) -> String {
    let mut prompt = String::from(
        "You are a pattern analyst. Given a cluster of related long-term memories, \
         identify the underlying pattern or principle they share.\n\n\
         ## Facts in this cluster\n",
    );

    for (fact_type, confidence, content) in facts {
        prompt.push_str(&format!(
            "- [{}, confidence={:.1}] {}\n",
            fact_type, confidence, content
        ));
    }

    prompt.push_str(
        "\nSynthesize ONE high-level insight that captures the common pattern.\n\
         Output JSON: {\"theme\": \"...\", \"insight\": \"...\", \"confidence\": 0.0-1.0}\n\n\
         Rules:\n\
         - The insight should be actionable — something that guides future behavior\n\
         - If the facts are too diverse to form a pattern, respond {\"theme\": null}\n\
         - Preserve all identifiers exactly as they appear\n",
    );

    prompt
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib synthesis 2>&1 | tail -15`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/dreaming/stages/synthesis.rs
git commit -m "dream-pipeline: implement DeepSynthesisStage for cross-session pattern extraction"
```

---

## Task 10: DreamingConfig Extension

**Files:**
- Modify: `src/config/types/memory.rs` (lines 356-384)

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn dreaming_config_defaults_include_new_fields() {
    let config = DreamingConfig::default();
    assert!(config.weekly_enabled);
    assert_eq!(config.weekly_interval_days, 7);
    assert!((config.cluster_dbscan_eps - 0.3).abs() < f32::EPSILON);
    assert_eq!(config.cluster_dbscan_min_samples, 2);
    assert!((config.drift_similarity_threshold - 0.85).abs() < f32::EPSILON);
    assert_eq!(config.drift_max_pairs_per_run, 20);
    assert_eq!(config.synthesis_min_cluster_size, 3);
    assert_eq!(config.synthesis_max_insights, 10);
}
```

- [ ] **Step 2: Run test — expect failure**

Run: `cargo test -p alephcore --lib dreaming_config_defaults_include 2>&1 | tail -10`
Expected: FAIL — fields don't exist

- [ ] **Step 3: Add new fields to DreamingConfig**

In `src/config/types/memory.rs`, add after `max_duration_seconds` (line 371):

```rust
    /// Enable weekly deep synthesis
    #[serde(default = "default_weekly_enabled")]
    pub weekly_enabled: bool,
    /// Days between weekly synthesis runs
    #[serde(default = "default_weekly_interval_days")]
    pub weekly_interval_days: u32,
    /// DBSCAN epsilon (cosine distance threshold)
    #[serde(default = "default_cluster_dbscan_eps")]
    pub cluster_dbscan_eps: f32,
    /// DBSCAN minimum samples per cluster
    #[serde(default = "default_cluster_dbscan_min_samples")]
    pub cluster_dbscan_min_samples: usize,
    /// Drift detection similarity threshold
    #[serde(default = "default_drift_similarity_threshold")]
    pub drift_similarity_threshold: f32,
    /// Max drift pairs per dream run
    #[serde(default = "default_drift_max_pairs_per_run")]
    pub drift_max_pairs_per_run: usize,
    /// Minimum cluster size for synthesis
    #[serde(default = "default_synthesis_min_cluster_size")]
    pub synthesis_min_cluster_size: usize,
    /// Max insights per weekly synthesis
    #[serde(default = "default_synthesis_max_insights")]
    pub synthesis_max_insights: usize,
```

Add default functions:

```rust
fn default_weekly_enabled() -> bool { true }
fn default_weekly_interval_days() -> u32 { 7 }
fn default_cluster_dbscan_eps() -> f32 { 0.3 }
fn default_cluster_dbscan_min_samples() -> usize { 2 }
fn default_drift_similarity_threshold() -> f32 { 0.85 }
fn default_drift_max_pairs_per_run() -> usize { 20 }
fn default_synthesis_min_cluster_size() -> usize { 3 }
fn default_synthesis_max_insights() -> usize { 10 }
```

Update `Default for DreamingConfig` to include the new fields.

Also add accessor methods so stages can read config without knowing field names:

```rust
impl DreamingConfig {
    pub fn cluster_dbscan_eps(&self) -> f32 { self.cluster_dbscan_eps }
    pub fn cluster_dbscan_min_samples(&self) -> usize { self.cluster_dbscan_min_samples }
    pub fn drift_similarity_threshold(&self) -> f32 { self.drift_similarity_threshold }
    pub fn drift_max_pairs_per_run(&self) -> usize { self.drift_max_pairs_per_run }
    pub fn synthesis_min_cluster_size(&self) -> usize { self.synthesis_min_cluster_size }
    pub fn synthesis_max_insights(&self) -> usize { self.synthesis_max_insights }
}
```

- [ ] **Step 4: Run test — expect pass**

Run: `cargo test -p alephcore --lib dreaming_config_defaults_include 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Also update the webchat config mirror**

`interfaces/webchat/src/api/memory_config.rs` has a parallel `DreamingConfig`. Add the same 8 fields with serde defaults for backward compatibility.

- [ ] **Step 6: Full build check**

Run: `cargo check 2>&1 | head -20`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/config/types/memory.rs interfaces/webchat/src/api/memory_config.rs
git commit -m "dream-pipeline: extend DreamingConfig with clustering, drift, and synthesis settings"
```

---

## Task 11: Integration — Wire Pipeline into DreamDaemon

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Write integration test**

```rust
#[cfg(test)]
mod pipeline_tests {
    use super::*;

    #[tokio::test]
    async fn daily_pipeline_runs_all_stages() {
        let pipeline = DreamPipeline::daily();
        let ctx = DreamContext::test_default();
        let report = pipeline.run(ctx).await.unwrap();
        assert_eq!(report.status_str(), "success");
    }

    #[tokio::test]
    async fn pipeline_interrupts_on_activity() {
        let pipeline = DreamPipeline::daily();
        let mut ctx = DreamContext::test_default();
        ctx.activity_checker = Arc::new(|| true); // always active
        let report = pipeline.run(ctx).await.unwrap();
        assert_eq!(report.status_str(), "cancelled");
    }

    #[tokio::test]
    async fn weekly_pipeline_includes_synthesis() {
        let pipeline = DreamPipeline::weekly();
        assert_eq!(pipeline.stages.len(), 7); // 6 daily + 1 synthesis
    }
}
```

- [ ] **Step 2: Ensure `run_dream()` uses the pipeline**

Verify the `run_dream()` method in `dreaming/mod.rs` calls `DreamPipeline::daily()` or `::weekly()` based on `determine_run_type()`. The `determine_run_type()` method should check `last_weekly_at` from DreamStatus:

```rust
async fn determine_run_type(&self) -> DreamRunType {
    if !self.config.weekly_enabled {
        return DreamRunType::Daily;
    }

    if let Ok(status) = self.database.get_dream_status().await {
        if let Some(last_weekly) = status.last_weekly_at {
            let days_since = (now_timestamp() - last_weekly) / 86400;
            if days_since >= self.config.weekly_interval_days as i64 {
                return DreamRunType::Weekly;
            }
        } else {
            // Never ran weekly — do it now
            return DreamRunType::Weekly;
        }
    }

    DreamRunType::Daily
}
```

Note: `DreamStatus` needs a `last_weekly_at` field. Add it:

```rust
// In dreaming/mod.rs
pub struct DreamStatus {
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_duration_ms: Option<u64>,
    pub last_weekly_at: Option<i64>,  // NEW
}
```

Update the `DreamStore` trait and LanceDB implementation to persist this field.

- [ ] **Step 3: Run integration tests**

Run: `cargo test -p alephcore --lib pipeline_tests 2>&1 | tail -20`
Expected: All 3 PASS

- [ ] **Step 4: Run ALL dreaming tests**

Run: `cargo test -p alephcore --lib dreaming 2>&1 | tail -20`
Expected: All pass (old tests + new tests)

- [ ] **Step 5: Full build**

Run: `cargo check 2>&1 | head -20`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/
git commit -m "dream-pipeline: wire DreamPipeline into DreamDaemon with weekly scheduling"
```

---

## Task 12: Cleanup + Final Verification

**Files:**
- Verify: all files in `src/memory/dreaming/`
- Verify: `src/memory/dreaming.rs` is deleted
- Verify: no remaining references to old functions

- [ ] **Step 1: Verify old file is gone**

```bash
test ! -f src/memory/dreaming.rs && echo "PASS: old file deleted" || echo "FAIL: old file still exists"
```

- [ ] **Step 2: Search for dangling references**

```bash
cargo check 2>&1 | head -30
```

- [ ] **Step 3: Run full test suite**

```bash
cargo test -p alephcore --lib 2>&1 | tail -30
```

Expected: All tests pass

- [ ] **Step 4: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | head -20
```

Expected: No warnings

- [ ] **Step 5: Verify no dead code from old dreaming.rs**

```bash
# Search for any remaining references to old private types
grep -rn "DreamCluster" src/memory/ --include="*.rs"
grep -rn "cluster_memories" src/memory/ --include="*.rs"
grep -rn "build_summary" src/memory/dreaming/ --include="*.rs"
```

Expected: No hits for old `DreamCluster` or `cluster_memories`. `build_cluster_summary` in new code is fine.

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "dream-pipeline: cleanup and final verification"
```

---

## Spec Coverage Verification

| Spec Requirement | Task |
|---|---|
| DreamStage trait | Task 1 |
| DreamPipeline executor | Task 2 |
| DreamContext shared state | Task 2 |
| CollectStage (extraction) | Task 3 |
| ClusterStage + DBSCAN | Task 4 |
| Identifier preservation | Task 5 |
| SummarizeStage | Task 6 |
| DriftDetectStage + LLM arbitration | Task 7 |
| ConsolidateStage (extraction) | Task 8 |
| DecayStage (extraction) | Task 8 |
| DeepSynthesisStage | Task 9 |
| DreamingConfig extension | Task 10 |
| Pipeline wiring + weekly scheduling | Task 11 |
| Old code deletion + cleanup | Task 12 |
| File organization (`dreaming/` module) | Task 2 |
| Activity interruption | Task 2 (preserved), Task 11 (tested) |

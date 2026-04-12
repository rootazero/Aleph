# Memory Logic Chain Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix disconnected implementations, delete dead code, and wire the hybrid retrieval pipeline in `src/memory/`.

**Architecture:** 5-layer fix executed as L2→L1→L3→L4→L5. Each task is independently compilable. Dead code cleanup first (L2) reduces compilation time and avoids conflicts with subsequent layers.

**Tech Stack:** Rust, SQLite (rusqlite), async_trait, tokio, serde_json

---

## Task 1: Delete Cognitive Modules (evolution, consolidation, reflection, promotion)

**Files:**
- Delete: `src/memory/evolution/` (entire directory: `mod.rs`, `chain.rs`, `detector.rs`, `resolver.rs`, `tests.rs`)
- Delete: `src/memory/consolidation/` (entire directory: `mod.rs`, `analyzer.rs`, `profile.rs`, `tests.rs`)
- Delete: `src/memory/reflection/` (entire directory: `mod.rs`, `mapper.rs`, `parser.rs`, `prompt.rs`, `service.rs`)
- Delete: `src/memory/promotion.rs`
- Modify: `src/memory/mod.rs` — remove `pub mod` and `pub use` lines

- [ ] **Step 1: Remove pub mod and pub use declarations from mod.rs**

In `src/memory/mod.rs`, remove these lines:

```rust
// Remove these pub mod declarations:
pub mod consolidation;
pub mod evolution;
pub mod promotion;
pub mod reflection;

// Remove these pub use blocks:
pub use consolidation::{
    ConsolidatedFact, ConsolidationAnalyzer, ConsolidationConfig, FrequentFact, ProfileCategory,
    UserProfile,
};
pub use evolution::{
    ContradictionDetector, EvolutionChain, EvolutionNode, EvolutionResolver, FactEvolution,
    ResolutionStrategy,
};
```

Also remove any `pub use` for `promotion` (check if exists).

- [ ] **Step 2: Delete the directories and file**

```bash
rm -rf src/memory/evolution/
rm -rf src/memory/consolidation/
rm -rf src/memory/reflection/
rm src/memory/promotion.rs
```

- [ ] **Step 3: Compile to verify no breakage**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: successful compilation. If any external file imports these types, fix the import (should be none based on grep analysis).

- [ ] **Step 4: Commit**

```bash
git add -A src/memory/evolution/ src/memory/consolidation/ src/memory/reflection/ src/memory/promotion.rs src/memory/mod.rs
git commit -m "memory: remove dead cognitive modules (evolution, consolidation, reflection, promotion)

These modules were fully implemented but never called from any production
path. Dream stages (DriftDetectStage, ConsolidateStage) reimplement the
same logic inline. Removing to align code with actual behavior."
```

---

## Task 2: Delete Auxiliary Dead Modules (backup, cleanup, performance_monitor, lazy_decay, adaptive_retrieval)

**Files:**
- Delete: `src/memory/backup.rs`
- Delete: `src/memory/cleanup.rs`
- Delete: `src/memory/performance_monitor.rs`
- Delete: `src/memory/lazy_decay.rs`
- Delete: `src/memory/adaptive_retrieval.rs`
- Modify: `src/memory/mod.rs` — remove `pub mod` and `pub use` lines

- [ ] **Step 1: Remove pub mod and pub use from mod.rs**

In `src/memory/mod.rs`, remove:

```rust
// Remove these pub mod declarations:
pub mod adaptive_retrieval;
pub mod backup;
pub mod cleanup;
pub mod lazy_decay;
pub mod performance_monitor;

// Remove these pub use lines:
pub use adaptive_retrieval::{AdaptiveRetrievalConfig, AdaptiveRetrievalGate, RetrievalDecision};
pub use backup::MemoryBackupService;
pub use cleanup::CleanupService;
pub use lazy_decay::{DecayEvaluation, LazyDecayEngine};
```

- [ ] **Step 2: Delete the files**

```bash
rm src/memory/backup.rs
rm src/memory/cleanup.rs
rm src/memory/performance_monitor.rs
rm src/memory/lazy_decay.rs
rm src/memory/adaptive_retrieval.rs
```

- [ ] **Step 3: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: successful compilation.

- [ ] **Step 4: Commit**

```bash
git add -A src/memory/backup.rs src/memory/cleanup.rs src/memory/performance_monitor.rs src/memory/lazy_decay.rs src/memory/adaptive_retrieval.rs src/memory/mod.rs
git commit -m "memory: remove unused auxiliary modules (backup, cleanup, perf_monitor, lazy_decay, adaptive_retrieval)

All had zero production callers. Backup and cleanup will be redesigned
as a dedicated project. AdaptiveRetrievalGate was never wired into any
retrieval path."
```

---

## Task 3: Delete VFS Migration, Compression Archival, Clean Up Orphans

**Files:**
- Delete: `src/memory/vfs/migration.rs`
- Delete: `src/memory/compression/archival.rs`
- Modify: `src/memory/vfs/mod.rs` — remove `pub mod migration` and re-export
- Modify: `src/memory/compression/mod.rs` — remove `mod archival` and re-exports
- Modify: `src/memory/mod.rs` — remove `migrate_existing_facts_to_paths` from vfs re-export
- Modify: `src/memory/ingestion.rs` — remove `_noise_filter` field
- Modify: `src/memory/store/mod.rs` — remove `MemoryEventStore` trait (lines 437-499)

- [ ] **Step 1: Remove vfs/migration.rs**

Delete `src/memory/vfs/migration.rs`.

In `src/memory/vfs/mod.rs`, remove:
```rust
pub mod migration;
```

In `src/memory/mod.rs`, remove `migrate_existing_facts_to_paths` from the vfs re-export line:
```rust
// Change from:
pub use vfs::{
    bootstrap_agent_context, compute_directory_hash, migrate_existing_facts_to_paths, L1Generator,
};
// Change to:
pub use vfs::{
    bootstrap_agent_context, compute_directory_hash, L1Generator,
};
```

- [ ] **Step 2: Remove compression/archival.rs**

Delete `src/memory/compression/archival.rs`.

In `src/memory/compression/mod.rs`, remove:
```rust
mod archival;
pub use archival::{ArchivalConfig, ArchivalResult, ArchivalService};
```

- [ ] **Step 3: Clean _noise_filter in ingestion.rs**

In `src/memory/ingestion.rs`, remove the `_noise_filter` field and its construction:

```rust
// Remove from struct:
//     _noise_filter: NoiseFilter,

// Remove from new():
//     let noise_filter = NoiseFilter::new(config.noise_filter.clone());
// And:
//     _noise_filter: noise_filter,

// Remove unused import:
//     use crate::memory::noise_filter::NoiseFilter;
```

The struct becomes:
```rust
#[derive(Clone)]
pub struct MemoryIngestion {
    _database: MemoryBackend,
    _embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
}
```

And `new()`:
```rust
pub fn new(
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
) -> Self {
    ensure_dream_daemon(database.clone(), Arc::clone(&config), None);
    Self {
        _database: database,
        _embedder: embedder,
        config,
    }
}
```

- [ ] **Step 4: Remove MemoryEventStore trait from store/mod.rs**

In `src/memory/store/mod.rs`, delete the entire `MemoryEventStore` trait block (lines 437-499):

```rust
// Delete this entire section:
// ---------------------------------------------------------------------------
// MemoryEventStore -- Event sourcing persistence trait
// ---------------------------------------------------------------------------
// ... through to the closing brace of the trait
```

Also remove the import it uses if now unused:
```rust
// If this import is only used by MemoryEventStore, remove it:
// use crate::memory::events::{MemoryEvent, MemoryEventEnvelope};
```

- [ ] **Step 5: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: successful compilation.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "memory: remove vfs/migration, compression/archival, MemoryEventStore trait, noise_filter field

- vfs/migration.rs: zero callers
- compression/archival.rs: zero callers
- MemoryEventStore trait: no type implements it (StateDatabase has
  inherent methods with different names)
- ingestion._noise_filter: instantiated but never used"
```

---

## Task 4: Implement DreamStore Persistence

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` — add DDL for dream_status and daily_insights tables
- Modify: `src/memory/store/sqlite/sessions.rs` — replace no-op implementations

- [ ] **Step 1: Add DDL constants in schema.rs**

In `src/memory/store/sqlite/schema.rs`, add after `DREAM_REPORTS_DDL`:

```rust
// ---------------------------------------------------------------------------
// Dream status table (singleton row for daemon state)
// ---------------------------------------------------------------------------

const DREAM_STATUS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_status (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at      INTEGER,
    last_status      TEXT,
    last_duration_ms INTEGER
);
"#;

// ---------------------------------------------------------------------------
// Daily insights table
// ---------------------------------------------------------------------------

const DAILY_INSIGHTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS daily_insights (
    date                 TEXT PRIMARY KEY,
    content              TEXT NOT NULL,
    source_memory_count  INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL
);
"#;
```

- [ ] **Step 2: Add table creation to init_schema()**

In `init_schema()`, add after the `DREAM_REPORTS_DDL` block:

```rust
    conn.execute_batch(DREAM_STATUS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_status table: {e}")))?;

    conn.execute_batch(DAILY_INSIGHTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create daily_insights table: {e}")))?;
```

- [ ] **Step 3: Implement DreamStore in sessions.rs**

Replace the entire content of `src/memory/store/sqlite/sessions.rs`:

```rust
//! DreamStore and CompressionStore implementations for SqliteMemoryBackend.

use async_trait::async_trait;
use rusqlite::params;

use crate::error::AlephError;
use crate::memory::context::CompressionSession;
use crate::memory::dreaming::{DailyInsight, DreamStatus};
use crate::memory::store::{CompressionStore, DreamStore};

use super::SqliteMemoryBackend;

// ============================================================================
// DreamStore implementation
// ============================================================================

#[async_trait]
impl DreamStore for SqliteMemoryBackend {
    async fn get_dream_status(&self) -> Result<DreamStatus, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT last_run_at, last_status, last_duration_ms FROM dream_status WHERE id = 1")
            .map_err(|e| AlephError::other(format!("Failed to prepare dream_status query: {e}")))?;

        let result = stmt.query_row([], |row| {
            Ok(DreamStatus {
                last_run_at: row.get(0)?,
                last_status: row.get(1)?,
                last_duration_ms: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
            })
        });

        match result {
            Ok(status) => Ok(status),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(DreamStatus::default()),
            Err(e) => Err(AlephError::other(format!("Failed to get dream status: {e}"))),
        }
    }

    async fn set_dream_status(&self, status: DreamStatus) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO dream_status (id, last_run_at, last_status, last_duration_ms)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 last_run_at = excluded.last_run_at,
                 last_status = excluded.last_status,
                 last_duration_ms = excluded.last_duration_ms",
            params![
                status.last_run_at,
                status.last_status,
                status.last_duration_ms.map(|v| v as i64),
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to set dream status: {e}")))?;
        Ok(())
    }

    async fn upsert_daily_insight(&self, insight: DailyInsight) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO daily_insights (date, content, source_memory_count, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(date) DO UPDATE SET
                 content = excluded.content,
                 source_memory_count = excluded.source_memory_count,
                 created_at = excluded.created_at",
            params![
                insight.date,
                insight.content,
                insight.source_memory_count,
                insight.created_at,
            ],
        )
        .map_err(|e| AlephError::other(format!("Failed to upsert daily insight: {e}")))?;
        Ok(())
    }

    async fn get_daily_insight(&self, date: &str) -> Result<Option<DailyInsight>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                "SELECT date, content, source_memory_count, created_at
                 FROM daily_insights WHERE date = ?1",
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare daily_insight query: {e}")))?;

        let result = stmt.query_row(params![date], |row| {
            Ok(DailyInsight {
                date: row.get(0)?,
                content: row.get(1)?,
                source_memory_count: row.get(2)?,
                created_at: row.get(3)?,
            })
        });

        match result {
            Ok(insight) => Ok(Some(insight)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::other(format!("Failed to get daily insight: {e}"))),
        }
    }
}

// ============================================================================
// CompressionStore implementation
// ============================================================================

#[async_trait]
impl CompressionStore for SqliteMemoryBackend {
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value) VALUES ('last_timestamp', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![timestamp.to_string()],
        )
        .map_err(|e| AlephError::other(format!("Failed to set compression timestamp: {e}")))?;
        Ok(())
    }

    async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = 'last_timestamp'")
            .map_err(|e| {
                AlephError::other(format!("Failed to prepare compression timestamp query: {e}"))
            })?;

        let result = stmt.query_row([], |row| {
            let val: String = row.get(0)?;
            Ok(val)
        });

        match result {
            Ok(val) => Ok(val.parse::<i64>().ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::other(format!(
                "Failed to get compression timestamp: {e}"
            ))),
        }
    }

    async fn record_compression_session(
        &self,
        session: &CompressionSession,
    ) -> Result<(), AlephError> {
        let json = serde_json::to_string(session)
            .map_err(|e| AlephError::other(format!("Failed to serialize compression session: {e}")))?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![format!("session_{}", session.session_id), json],
        )
        .map_err(|e| AlephError::other(format!("Failed to record compression session: {e}")))?;
        Ok(())
    }
}
```

- [ ] **Step 4: Add compression_metadata DDL to schema.rs**

In `src/memory/store/sqlite/schema.rs`, add after `DAILY_INSIGHTS_DDL`:

```rust
// ---------------------------------------------------------------------------
// Compression metadata table (key-value store)
// ---------------------------------------------------------------------------

const COMPRESSION_METADATA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS compression_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;
```

And in `init_schema()`:
```rust
    conn.execute_batch(COMPRESSION_METADATA_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create compression_metadata table: {e}")))?;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::store::sqlite -- --test-threads=1 2>&1 | tail -20
```

Expected: all existing tests pass, schema initialization includes new tables.

- [ ] **Step 6: Compile full crate**

```bash
cargo check -p alephcore 2>&1 | head -30
```

- [ ] **Step 7: Commit**

```bash
git add src/memory/store/sqlite/sessions.rs src/memory/store/sqlite/schema.rs
git commit -m "memory: implement DreamStore and CompressionStore persistence

Replace no-op implementations with real SQLite persistence:
- dream_status table: singleton row tracks last run time/status
- daily_insights table: stores dream daemon insight summaries
- compression_metadata table: key-value store for compression timestamps
  and session audit records

Fixes: dream status loss on restart, daily insights silently discarded,
compression re-processing after restart."
```

---

## Task 5: Fix MemoryRetrieval Stub

**Files:**
- Modify: `src/memory/retrieval.rs` — delegate to FactRetrieval instead of returning empty Vec

- [ ] **Step 1: Update MemoryRetrieval to delegate to FactRetrieval**

Replace `src/memory/retrieval.rs` content:

```rust
/// Memory retrieval module
///
/// Delegates to FactRetrieval for Layer 2 (compressed facts) search.
/// Retained for API compatibility with MemoryStrategy callers.
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry, MemoryFact};
use crate::memory::dreaming::record_activity;
use crate::memory::fact_retrieval::FactRetrieval;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::debug;

/// Memory retrieval service — delegates to FactRetrieval
#[derive(Clone)]
pub struct MemoryRetrieval {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
}

impl MemoryRetrieval {
    /// Create new retrieval service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        Self {
            database,
            embedder,
            config,
        }
    }

    /// Retrieve memories for current context by delegating to FactRetrieval
    pub async fn retrieve_memories(
        &self,
        _context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        let fact_retrieval = FactRetrieval::new(
            self.database.clone(),
            self.embedder.clone(),
            self.config.clone(),
        );

        let result = fact_retrieval
            .retrieve(query, self.config.max_context_items)
            .await?;

        Ok(result.facts.into_iter().map(|sf| fact_to_memory_entry(&sf.fact)).collect())
    }

    /// Retrieve memories with custom limit
    pub async fn retrieve_memories_with_limit(
        &self,
        _context: &ContextAnchor,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        let fact_retrieval = FactRetrieval::new(
            self.database.clone(),
            self.embedder.clone(),
            self.config.clone(),
        );

        let result = fact_retrieval.retrieve(query, limit).await?;

        Ok(result.facts.into_iter().map(|sf| fact_to_memory_entry(&sf.fact)).collect())
    }
}

/// Convert a MemoryFact to a MemoryEntry for backward compatibility.
fn fact_to_memory_entry(fact: &MemoryFact) -> MemoryEntry {
    MemoryEntry {
        id: fact.id.clone(),
        input: fact.content.clone(),
        output: String::new(),
        context: ContextAnchor::now(String::new()),
        embedding: None,
        created_at: fact.created_at,
        namespace: fact.namespace.clone(),
        agent: fact.agent.clone(),
    }
}
```

Note: The exact fields of `MemoryEntry` and `FactRetrieval::retrieve` return type may need adjustment. Check the actual signatures in `src/memory/context/mod.rs` and `src/memory/fact_retrieval.rs` and adapt accordingly. The key change is: **delegate to FactRetrieval instead of returning empty Vec**.

- [ ] **Step 2: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Fix any type mismatches between `MemoryEntry` and `MemoryFact` fields.

- [ ] **Step 3: Commit**

```bash
git add src/memory/retrieval.rs
git commit -m "memory: fix MemoryRetrieval stub to delegate to FactRetrieval

Previously returned empty Vec, causing MemoryStrategy's AI retrieval
to always receive zero candidates. Now delegates to FactRetrieval for
actual vector search results."
```

---

## Task 6: Wire HybridRetrieval into Production Retrieval Path

**Files:**
- Modify: `src/builtin_tools/memory_search.rs` — construct HybridRetrieval
- Modify: `src/memory/fact_retrieval.rs` — add hybrid mode option

- [ ] **Step 1: Read current memory_search.rs and fact_retrieval.rs**

Read both files to understand the current construction and calling patterns:
```bash
# Read the files to understand current code structure
```

- [ ] **Step 2: Add HybridRetrieval construction in the retrieval path**

The exact change depends on the current code structure. The pattern is:

In the primary retrieval entry point (either `memory_search` tool's execute method or `FactRetrieval::retrieve`):

```rust
use crate::memory::hybrid_retrieval::hybrid::{HybridRetrieval, HybridSearchConfig};

// Replace direct vector_search call with HybridRetrieval
let hybrid = HybridRetrieval::with_defaults(database.clone());
let results = hybrid.search_facts(query, embedding, dim_hint, &filter, limit).await?;
```

`HybridRetrieval::with_defaults()` already constructs the `ScoringPipeline` internally, so no separate pipeline wiring is needed.

- [ ] **Step 3: Retain FactRetrieval as fallback**

Keep the existing `FactRetrieval` code path but gate it behind an embedder availability check:

```rust
// If embedder is unavailable, fall back to pure vector search via FactRetrieval
// Otherwise, use HybridRetrieval for vector + BM25 fusion
```

- [ ] **Step 4: Run existing HybridRetrieval tests to verify they still pass**

```bash
cargo test -p alephcore --lib memory::hybrid_retrieval -- --test-threads=1 2>&1 | tail -20
```

- [ ] **Step 5: Compile full crate**

```bash
cargo check -p alephcore 2>&1 | head -30
```

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/memory_search.rs src/memory/fact_retrieval.rs
git commit -m "memory: wire HybridRetrieval into production retrieval path

Production retrieval now uses vector + BM25 fusion (RRF scoring) via
HybridRetrieval instead of pure vector search. ScoringPipeline with
7 stages (recency_boost, time_decay, etc.) is automatically applied.
FactRetrieval retained as fallback when embedder is unavailable."
```

---

## Task 7: Fix Dream Pipeline — Remove Stub Stages

**Files:**
- Modify: `src/memory/dreaming/mod.rs` — remove WikiIngestStage and TunnelDiscoveryStage from pipeline

- [ ] **Step 1: Update DreamPipeline::daily()**

In `src/memory/dreaming/mod.rs`, change `DreamPipeline::daily()` from:

```rust
    pub fn daily() -> Self {
        Self::new()
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(WikiIngestStage)
            .stage(WikiLintStage)
            .stage(TunnelDiscoveryStage)
            .stage(DecayStage)
    }
```

To:

```rust
    pub fn daily() -> Self {
        Self::new()
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(WikiLintStage)
            .stage(DecayStage)
    }
```

- [ ] **Step 2: Update test assertions**

In the same file, update test expectations:

```rust
// In test_pipeline_builder:
assert_eq!(pipeline.stages.len(), 5); // was 7

// In test_pipeline_weekly_has_eight_stages:
assert_eq!(pipeline.stages.len(), 6); // was 8 (5 daily + DeepSynthesisStage)

// In daily_pipeline_has_seven_stages (rename to daily_pipeline_has_five_stages):
assert_eq!(pipeline.stages.len(), 5); // was 7

// In weekly_pipeline_has_eight_stages (rename to weekly_pipeline_has_six_stages):
assert_eq!(pipeline.stages.len(), 6); // was 8
```

- [ ] **Step 3: Run dream pipeline tests**

```bash
cargo test -p alephcore --lib memory::dreaming -- --test-threads=1 2>&1 | tail -20
```

Expected: all tests pass with updated counts.

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "memory: remove WikiIngestStage and TunnelDiscoveryStage from dream pipeline

WikiIngestStage: execute only logs 'LLM ingestion pending', never
creates wiki pages. Kept file for future completion.

TunnelDiscoveryStage: should_run checks tunnel_pending which nothing
ever sets. Kept file for future tunnel write-path.

Daily pipeline: 7 stages -> 5 stages. All remaining stages verified
functional."
```

---

## Task 8: Fix WikiLintStage — Persist Report

**Files:**
- Modify: `src/memory/dreaming/stages/wiki_lint.rs` — store report on DreamContext
- Modify: `src/memory/dreaming/mod.rs` — add `wiki_lint_report` field to DreamContext
- Modify: `src/memory/dreaming/report.rs` — add `wiki_lint_summary` to DreamReport
- Modify: `src/memory/store/sqlite/dream_reports.rs` — persist lint summary

- [ ] **Step 1: Add wiki_lint_report field to DreamContext**

In `src/memory/dreaming/mod.rs`, add to the `DreamContext` struct:

```rust
    /// Output: wiki lint report populated by WikiLintStage.
    pub wiki_lint_report: Option<String>,
```

Initialize it as `None` in `run_dream()` where `DreamContext` is constructed:
```rust
    wiki_lint_report: None,
```

- [ ] **Step 2: Store report in wiki_lint.rs**

In `src/memory/dreaming/stages/wiki_lint.rs`, after the info! log at line 83, add before `Ok(ctx)`:

```rust
        // Persist report as JSON on the context
        let mut ctx = ctx;
        if !report.broken_links.is_empty() || !report.orphan_pages.is_empty() {
            ctx.wiki_lint_report = serde_json::to_string(&report).ok();
        }

        Ok(ctx)
```

Change the function signature to accept `ctx: DreamContext` as mutable by changing:
```rust
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
```
(It already takes ownership, so `let mut ctx = ctx;` works.)

- [ ] **Step 3: Add wiki_lint_summary to DreamReport**

In `src/memory/dreaming/report.rs`, add to `DreamReport`:

```rust
    pub wiki_lint_summary: Option<String>,
```

In `interrupted()` and `completed_default()`, add:
```rust
    wiki_lint_summary: ctx.wiki_lint_report.clone(),
```

- [ ] **Step 4: Persist in dream_reports**

In `src/memory/dreaming/mod.rs`, in the `run_dream()` method where `PersistedDreamReport` is constructed, the lint summary can be stored in the existing `errors` field or you can add a comment that it flows through `DreamReport`. The existing `PersistedDreamReport` already has an `errors: Option<String>` field that can carry this.

Update the persisted report construction:
```rust
    errors: report.wiki_lint_summary.clone(),
```

(This reuses the existing `errors` field which was always `None`. If a dedicated field is preferred, add it to the `dream_reports` schema.)

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib memory::dreaming -- --test-threads=1 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/mod.rs src/memory/dreaming/stages/wiki_lint.rs src/memory/dreaming/report.rs
git commit -m "memory: persist WikiLintStage report instead of discarding

WikiLintReport (broken links, orphan pages) was built as a local
variable then dropped. Now serialized to JSON and stored on DreamContext,
flowing through to DreamReport and PersistedDreamReport for audit."
```

---

## Task 9: Update mod.rs Comment and Documentation

**Files:**
- Modify: `src/memory/mod.rs` — fix module doc comment
- Modify: `docs/reference/MEMORY_SYSTEM.md` — sync with reality

- [ ] **Step 1: Fix mod.rs doc comment**

Replace the top of `src/memory/mod.rs`:

```rust
//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (window_title + session_id).
//!
//! ## Architecture
//!
//! - **Storage**: SQLite + sqlite-vec via `store::sqlite::SqliteMemoryBackend`
//!
//! ## Storage Traits
//!
//! - `MemoryStore`: Fact CRUD, vector search, path operations
//! - `GraphStore`: Entity relationship graph operations
//! - `DreamStore`, `CompressionStore`: Specialized operations
```

(Changed "SQLite + sqlite-vec" from the stale LanceDB reference.)

- [ ] **Step 2: Update MEMORY_SYSTEM.md**

Key changes in `docs/reference/MEMORY_SYSTEM.md`:

1. Update the Architecture diagram description to note HybridRetrieval as the production path
2. Remove the "Retention Policies" section referencing `retention.rs` if it doesn't exist
3. Remove/update references to deleted modules:
   - Remove "Knowledge Exploration with RippleTask" usage example (RippleTask is disconnected)
   - Remove "Contradiction Detection" usage example (ContradictionDetector deleted)
   - Remove "User Profile Distillation" usage example (ConsolidationAnalyzer deleted)
4. Update DreamDaemon section: "5 stages" not "7 stages"
5. Add note under "Memory System Evolution" section marking rerank, query_expander, event sourcing as "implemented / pending connection"

- [ ] **Step 3: Commit**

```bash
git add src/memory/mod.rs docs/reference/MEMORY_SYSTEM.md
git commit -m "docs: sync MEMORY_SYSTEM.md and mod.rs comments with actual code state

- Fix storage description (SQLite, not LanceDB)
- Remove references to deleted modules
- Update DreamPipeline to 5 stages
- Mark rerank/query_expander/event sourcing as pending connection"
```

---

## Task 10: Final Verification

- [ ] **Step 1: Full compilation**

```bash
cargo check -p alephcore 2>&1 | head -30
```

- [ ] **Step 2: Run all memory tests**

```bash
cargo test -p alephcore --lib memory:: -- --test-threads=1 2>&1 | tail -30
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30
```

- [ ] **Step 4: Verify line count reduction**

```bash
find src/memory -name '*.rs' | wc -l
# Expected: ~145 files (down from 166)

git diff --stat HEAD~9..HEAD
# Expected: significant net deletion
```

- [ ] **Step 5: Final commit (if any clippy fixes needed)**

```bash
git add -A
git commit -m "memory: fix clippy warnings after logic chain cleanup"
```

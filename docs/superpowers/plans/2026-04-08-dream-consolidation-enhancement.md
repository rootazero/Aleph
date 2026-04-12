# Dream Consolidation Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enhance Aleph's dreaming mechanism with recall signal tracking, 8-dimensional promotion scoring, dream audit reports, LLM-powered synthesis, and layered retrieval.

**Architecture:** Add a `recall_signals` table to track memory search hits. Replace the simple consolidation rule with an 8-dimensional scorer (including graph_centrality and cluster_cohesion unique to Aleph). Enhance `DeepSynthesisStage` with real LLM synthesis and deduplication. Add `dream_reports` table for audit. Implement layered retrieval separating synthesis background from query-relevant detail.

**Tech Stack:** Rust, SQLite (rusqlite), sqlite-vec, async_trait, tokio, serde_json, sha2, tracing

---

### Task 1: RecallSignalStore — Schema and Basic CRUD

**Files:**
- Create: `src/memory/store/sqlite/recall_signals.rs`
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/mod.rs`

- [ ] **Step 1: Add DDL to schema.rs**

```rust
// src/memory/store/sqlite/schema.rs — add after GRAPH_EDGES_DDL

const RECALL_SIGNALS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    fact_id     TEXT NOT NULL,
    query_hash  TEXT NOT NULL,
    query_text  TEXT NOT NULL,
    channel     TEXT NOT NULL DEFAULT 'unknown',
    score       REAL NOT NULL,
    session_id  TEXT,
    namespace   TEXT NOT NULL DEFAULT 'owner',
    created_at  INTEGER NOT NULL,
    day_bucket  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_recall_dedup
    ON recall_signals(fact_id, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_fact_id
    ON recall_signals(fact_id);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket
    ON recall_signals(day_bucket);
"#;
```

And add to `init_schema()`:

```rust
conn.execute_batch(RECALL_SIGNALS_DDL)
    .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;
```

- [ ] **Step 2: Create recall_signals.rs with types**

```rust
// src/memory/store/sqlite/recall_signals.rs
//! RecallSignalStore: tracks memory retrieval signals for promotion scoring.

use rusqlite::params;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AlephError;
use super::SqliteMemoryBackend;

/// Aggregated recall signals for a single fact.
#[derive(Debug, Clone)]
pub struct RecallAggregate {
    pub fact_id: String,
    pub signal_count: u32,
    pub total_score: f32,
    pub unique_queries: u32,
    pub unique_channels: u32,
    pub recall_days: u32,
    pub first_recall: i64,
    pub last_recall: i64,
}

/// A single retrieval hit to record.
pub struct RecallHit {
    pub fact_id: String,
    pub score: f32,
}

fn query_hash(query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(query.trim().to_lowercase().as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8]) // 16-char hex
}

fn today_bucket() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}
```

- [ ] **Step 3: Implement record_signals**

```rust
impl SqliteMemoryBackend {
    /// Record retrieval signals for search hits. Uses ON CONFLICT to deduplicate.
    pub fn record_signals(
        &self,
        query: &str,
        channel: &str,
        hits: &[RecallHit],
        session_id: Option<&str>,
        namespace: &str,
    ) -> Result<usize, AlephError> {
        if hits.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let hash = query_hash(query);
        let bucket = today_bucket();
        let now = chrono::Utc::now().timestamp();
        let mut inserted = 0usize;

        let mut stmt = conn.prepare_cached(
            "INSERT INTO recall_signals (id, fact_id, query_hash, query_text, channel, score, session_id, namespace, created_at, day_bucket)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(fact_id, query_hash, day_bucket, channel) DO NOTHING"
        ).map_err(|e| AlephError::internal(format!("prepare recall_signals insert: {e}")))?;

        for hit in hits {
            let id = Uuid::new_v4().to_string();
            let rows = stmt.execute(params![
                id, hit.fact_id, hash, query, channel, hit.score,
                session_id, namespace, now, bucket,
            ]).map_err(|e| AlephError::internal(format!("insert recall_signal: {e}")))?;
            inserted += rows;
        }

        Ok(inserted)
    }
}
```

- [ ] **Step 4: Implement aggregate_for_facts**

```rust
impl SqliteMemoryBackend {
    /// Aggregate recall signals for a batch of fact IDs. Single SQL query, no N+1.
    pub fn aggregate_for_facts(
        &self,
        fact_ids: &[String],
    ) -> Result<Vec<RecallAggregate>, AlephError> {
        if fact_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let placeholders: String = fact_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT fact_id,
                    COUNT(*) as signal_count,
                    SUM(score) as total_score,
                    COUNT(DISTINCT query_hash) as unique_queries,
                    COUNT(DISTINCT channel) as unique_channels,
                    COUNT(DISTINCT day_bucket) as recall_days,
                    MIN(created_at) as first_recall,
                    MAX(created_at) as last_recall
             FROM recall_signals
             WHERE fact_id IN ({placeholders})
             GROUP BY fact_id"
        );

        let mut stmt = conn.prepare(&sql)
            .map_err(|e| AlephError::internal(format!("prepare aggregate: {e}")))?;

        let params: Vec<&dyn rusqlite::types::ToSql> = fact_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(RecallAggregate {
                fact_id: row.get(0)?,
                signal_count: row.get(1)?,
                total_score: row.get(2)?,
                unique_queries: row.get(3)?,
                unique_channels: row.get(4)?,
                recall_days: row.get(5)?,
                first_recall: row.get(6)?,
                last_recall: row.get(7)?,
            })
        }).map_err(|e| AlephError::internal(format!("query aggregate: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::internal(format!("read aggregate row: {e}")))?);
        }

        Ok(results)
    }
}
```

- [ ] **Step 5: Implement cleanup_old_signals**

```rust
impl SqliteMemoryBackend {
    /// Remove signals older than retention_days (default 90).
    pub fn cleanup_old_signals(&self, retention_days: u32) -> Result<usize, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let cutoff = chrono::Utc::now().timestamp() - (retention_days as i64 * 86400);

        let deleted = conn.execute(
            "DELETE FROM recall_signals WHERE created_at < ?1",
            params![cutoff],
        ).map_err(|e| AlephError::internal(format!("cleanup recall_signals: {e}")))?;

        Ok(deleted)
    }
}
```

- [ ] **Step 6: Register module in mod.rs**

Add `pub mod recall_signals;` to `src/memory/store/sqlite/mod.rs` and make `RECALL_SIGNALS_DDL` public or call `init_schema` from within `schema.rs`.

- [ ] **Step 7: Write tests**

```rust
// At bottom of src/memory/store/sqlite/recall_signals.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;

    fn setup() -> Arc<SqliteMemoryBackend> {
        let tmp = tempfile::tempdir().expect("tempdir");
        Arc::new(SqliteMemoryBackend::new(tmp.path()).expect("backend"))
    }

    #[test]
    fn record_and_aggregate_signals() {
        let db = setup();
        let hits = vec![
            RecallHit { fact_id: "f1".into(), score: 0.9 },
            RecallHit { fact_id: "f2".into(), score: 0.7 },
        ];
        let inserted = db.record_signals("test query", "slack", &hits, None, "owner").unwrap();
        assert_eq!(inserted, 2);

        let agg = db.aggregate_for_facts(&["f1".into(), "f2".into()]).unwrap();
        assert_eq!(agg.len(), 2);

        let f1 = agg.iter().find(|a| a.fact_id == "f1").unwrap();
        assert_eq!(f1.signal_count, 1);
        assert!((f1.total_score - 0.9).abs() < 0.01);
    }

    #[test]
    fn dedup_same_query_same_day_same_channel() {
        let db = setup();
        let hits = vec![RecallHit { fact_id: "f1".into(), score: 0.8 }];
        db.record_signals("q1", "slack", &hits, None, "owner").unwrap();
        let inserted = db.record_signals("q1", "slack", &hits, None, "owner").unwrap();
        assert_eq!(inserted, 0); // deduplicated

        let agg = db.aggregate_for_facts(&["f1".into()]).unwrap();
        assert_eq!(agg[0].signal_count, 1);
    }

    #[test]
    fn different_channels_count_separately() {
        let db = setup();
        let hits = vec![RecallHit { fact_id: "f1".into(), score: 0.8 }];
        db.record_signals("q1", "slack", &hits, None, "owner").unwrap();
        db.record_signals("q1", "web", &hits, None, "owner").unwrap();

        let agg = db.aggregate_for_facts(&["f1".into()]).unwrap();
        assert_eq!(agg[0].signal_count, 2);
        assert_eq!(agg[0].unique_channels, 2);
    }

    #[test]
    fn cleanup_removes_old_signals() {
        let db = setup();
        let hits = vec![RecallHit { fact_id: "f1".into(), score: 0.5 }];
        db.record_signals("q", "web", &hits, None, "owner").unwrap();
        // retention_days=0 should clean everything
        let deleted = db.cleanup_old_signals(0).unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn aggregate_empty_ids_returns_empty() {
        let db = setup();
        let agg = db.aggregate_for_facts(&[]).unwrap();
        assert!(agg.is_empty());
    }
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p alephcore --lib recall_signals`
Expected: All 5 tests PASS

- [ ] **Step 9: Commit**

```bash
git add src/memory/store/sqlite/recall_signals.rs src/memory/store/sqlite/schema.rs src/memory/store/sqlite/mod.rs
git commit -m "memory: add RecallSignalStore for retrieval signal tracking"
```

---

### Task 2: Hook Signal Recording into Retrieval Path

**Files:**
- Modify: `src/memory/fact_retrieval.rs`

- [ ] **Step 1: Add signal recording to FactRetrieval**

In `src/memory/fact_retrieval.rs`, add a method that wraps retrieval + signal recording:

```rust
use crate::memory::store::sqlite::recall_signals::RecallHit;

impl FactRetrieval {
    /// Retrieve facts and asynchronously record recall signals.
    pub async fn retrieve_with_signals(
        &self,
        query: &str,
        namespace: &NamespaceScope,
        channel: &str,
        session_id: Option<&str>,
    ) -> Result<RetrievalResult, AlephError> {
        let result = self.retrieve(query, namespace).await?;

        if !result.facts.is_empty() {
            let hits: Vec<RecallHit> = result.facts.iter().map(|f| RecallHit {
                fact_id: f.id.clone(),
                score: f.confidence, // use retrieval score if available
            }).collect();

            let db = self.database.clone();
            let query_owned = query.to_string();
            let channel_owned = channel.to_string();
            let session_owned = session_id.map(|s| s.to_string());
            let ns = namespace.effective_namespace().to_string();

            tokio::spawn(async move {
                if let Err(e) = db.record_signals(
                    &query_owned,
                    &channel_owned,
                    &hits,
                    session_owned.as_deref(),
                    &ns,
                ) {
                    tracing::warn!("recall signal recording failed: {e}");
                }
            });
        }

        Ok(result)
    }
}
```

- [ ] **Step 2: Run existing tests to verify no breakage**

Run: `cargo test -p alephcore --lib fact_retrieval`
Expected: PASS (new method is additive, no existing signatures changed)

- [ ] **Step 3: Commit**

```bash
git add src/memory/fact_retrieval.rs
git commit -m "memory: hook recall signal recording into retrieval path"
```

---

### Task 3: PromotionScorer — 8-Dimensional Scoring Engine

**Files:**
- Create: `src/memory/consolidation/promotion_scorer.rs`
- Modify: `src/memory/consolidation/mod.rs`

- [ ] **Step 1: Write tests first**

```rust
// src/memory/consolidation/promotion_scorer.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one() {
        let scorer = PromotionScorer::default();
        let sum: f32 = scorer.weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001, "weights sum to {sum}, expected 1.0");
    }

    #[test]
    fn score_all_zeros_returns_zero() {
        let scorer = PromotionScorer::default();
        let dims = [0.0_f32; 8];
        assert!((scorer.score(&dims) - 0.0).abs() < 0.001);
    }

    #[test]
    fn score_all_ones_returns_one() {
        let scorer = PromotionScorer::default();
        let dims = [1.0_f32; 8];
        let s = scorer.score(&dims);
        assert!((s - 1.0).abs() < 0.001, "score={s}");
    }

    #[test]
    fn frequency_dimension() {
        // log1p(5) / log1p(10) ≈ 0.748
        let d = compute_frequency(5);
        assert!(d > 0.7 && d < 0.8, "frequency(5) = {d}");
    }

    #[test]
    fn relevance_dimension() {
        let d = compute_relevance(4.5, 5);
        assert!((d - 0.9).abs() < 0.01);
    }

    #[test]
    fn diversity_dimension() {
        let d = compute_diversity(3, 2);
        assert!((d - 0.6).abs() < 0.01); // max(3,2)/5 = 0.6
    }

    #[test]
    fn recency_dimension_fresh() {
        let d = compute_recency(0.0); // today
        assert!((d - 1.0).abs() < 0.01);
    }

    #[test]
    fn recency_dimension_14_days() {
        let d = compute_recency(14.0); // half-life
        assert!((d - 0.5).abs() < 0.05);
    }

    #[test]
    fn graph_centrality_dimension() {
        let d = compute_graph_centrality(4);
        // log1p(4) / log1p(8) ≈ 0.733
        assert!(d > 0.7 && d < 0.8, "centrality(4) = {d}");
    }

    #[test]
    fn cluster_cohesion_at_centroid() {
        let d = compute_cluster_cohesion(0.0, 1.0);
        assert!((d - 1.0).abs() < 0.01);
    }

    #[test]
    fn cluster_cohesion_at_edge() {
        let d = compute_cluster_cohesion(1.0, 1.0);
        assert!((d - 0.0).abs() < 0.01);
    }

    #[test]
    fn cluster_cohesion_no_cluster() {
        let d = compute_cluster_cohesion(0.0, 0.0);
        assert!((d - 0.0).abs() < 0.01); // radius=0 means no cluster
    }
}
```

- [ ] **Step 2: Run tests — they should FAIL**

Run: `cargo test -p alephcore --lib promotion_scorer`
Expected: FAIL — functions not defined

- [ ] **Step 3: Implement dimension functions**

```rust
// src/memory/consolidation/promotion_scorer.rs
//! 8-dimensional promotion scoring engine for STM → LTM consolidation.

/// Frequency: how often a fact has been recalled.
pub fn compute_frequency(signal_count: u32) -> f32 {
    ((signal_count as f32).ln_1p() / 10.0_f32.ln_1p()).clamp(0.0, 1.0)
}

/// Relevance: average retrieval score across recalls.
pub fn compute_relevance(total_score: f32, signal_count: u32) -> f32 {
    if signal_count == 0 { return 0.0; }
    (total_score / signal_count as f32).clamp(0.0, 1.0)
}

/// Diversity: breadth of query/channel contexts.
pub fn compute_diversity(unique_queries: u32, unique_channels: u32) -> f32 {
    (unique_queries.max(unique_channels) as f32 / 5.0).clamp(0.0, 1.0)
}

/// Recency: exponential decay with 14-day half-life.
pub fn compute_recency(days_since_access: f32) -> f32 {
    let lambda = 2.0_f32.ln() / 14.0;
    (-lambda * days_since_access).exp().clamp(0.0, 1.0)
}

/// Consolidation: how spread out recalls are over time.
pub fn compute_consolidation(recall_days: u32, span_days: f32) -> f32 {
    if recall_days <= 1 { return 0.2; }
    let spacing = ((recall_days as f32 - 1.0).ln_1p() / 4.0_f32.ln_1p()).clamp(0.0, 1.0);
    let span = (span_days / 7.0).clamp(0.0, 1.0);
    (0.55 * spacing + 0.45 * span).clamp(0.0, 1.0)
}

/// Conceptual: richness of fact type and path tags.
pub fn compute_conceptual(tag_count: u32) -> f32 {
    (tag_count as f32 / 6.0).clamp(0.0, 1.0)
}

/// Graph centrality: how connected this fact's entities are.
pub fn compute_graph_centrality(edge_count: u32) -> f32 {
    ((edge_count as f32).ln_1p() / 8.0_f32.ln_1p()).clamp(0.0, 1.0)
}

/// Cluster cohesion: proximity to cluster centroid.
pub fn compute_cluster_cohesion(dist_to_centroid: f32, cluster_radius: f32) -> f32 {
    if cluster_radius <= 0.0 { return 0.0; }
    (1.0 - dist_to_centroid / cluster_radius).clamp(0.0, 1.0)
}
```

- [ ] **Step 4: Implement PromotionScorer struct**

```rust
/// Thresholds for promotion eligibility.
#[derive(Debug, Clone)]
pub struct PromotionThresholds {
    pub min_score: f32,
    pub min_signal_count: u32,
    pub min_unique_queries: u32,
    pub min_age_hours: u64,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            min_score: 0.65,
            min_signal_count: 3,
            min_unique_queries: 2,
            min_age_hours: 24,
        }
    }
}

/// 8-dimensional promotion scorer.
#[derive(Debug, Clone)]
pub struct PromotionScorer {
    pub weights: [f32; 8],
    pub thresholds: PromotionThresholds,
}

impl Default for PromotionScorer {
    fn default() -> Self {
        Self {
            weights: [0.20, 0.22, 0.13, 0.12, 0.10, 0.05, 0.10, 0.08],
            thresholds: PromotionThresholds::default(),
        }
    }
}

impl PromotionScorer {
    /// Compute weighted score from 8 dimensions.
    pub fn score(&self, dims: &[f32; 8]) -> f32 {
        self.weights.iter().zip(dims).map(|(w, d)| w * d).sum()
    }

    /// Check if a fact should be promoted based on score and thresholds.
    pub fn should_promote(
        &self,
        score: f32,
        signal_count: u32,
        unique_queries: u32,
        age_hours: u64,
    ) -> bool {
        score >= self.thresholds.min_score
            && signal_count >= self.thresholds.min_signal_count
            && unique_queries >= self.thresholds.min_unique_queries
            && age_hours >= self.thresholds.min_age_hours
    }
}
```

- [ ] **Step 5: Register in consolidation/mod.rs**

Add to `src/memory/consolidation/mod.rs`:

```rust
pub mod promotion_scorer;
pub use promotion_scorer::{PromotionScorer, PromotionThresholds};
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib promotion_scorer`
Expected: All 12 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/memory/consolidation/promotion_scorer.rs src/memory/consolidation/mod.rs
git commit -m "memory: add 8-dimensional PromotionScorer for consolidation"
```

---

### Task 4: Graph Centrality Batch Query

**Files:**
- Modify: `src/memory/store/sqlite/graph.rs`

- [ ] **Step 1: Write test**

```rust
// Add to existing tests in graph.rs
#[test]
fn edge_count_for_entities_batch() {
    let db = setup(); // existing test helper
    // Insert test nodes and edges...
    let counts = db.edge_count_for_entities(&["entity1".into(), "entity2".into()], "default").unwrap();
    assert!(counts.contains_key("entity1"));
}
```

- [ ] **Step 2: Implement edge_count_for_entities**

```rust
impl SqliteMemoryBackend {
    /// Count edges for multiple entities in a single query.
    /// Returns HashMap<entity_name, edge_count>.
    pub fn edge_count_for_entities(
        &self,
        entity_names: &[String],
        workspace: &str,
    ) -> Result<std::collections::HashMap<String, u32>, AlephError> {
        if entity_names.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        // First resolve entity names to node IDs
        let placeholders: String = entity_names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT gn.name, COUNT(DISTINCT ge.id) as edge_count
             FROM graph_nodes gn
             LEFT JOIN graph_edges ge ON (ge.from_id = gn.id OR ge.to_id = gn.id)
                AND ge.agent = ?1
             WHERE gn.name IN ({placeholders})
                AND gn.agent = ?1
             GROUP BY gn.name"
        );

        let mut stmt = conn.prepare(&sql)
            .map_err(|e| AlephError::internal(format!("prepare edge_count: {e}")))?;

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(workspace.to_string()));
        for name in entity_names {
            all_params.push(Box::new(name.clone()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        }).map_err(|e| AlephError::internal(format!("query edge_count: {e}")))?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (name, count) = row.map_err(|e| AlephError::internal(format!("read edge_count row: {e}")))?;
            result.insert(name, count);
        }

        Ok(result)
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib graph`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/graph.rs
git commit -m "memory: add edge_count_for_entities batch query for graph centrality"
```

---

### Task 5: Rewrite ConsolidateStage with PromotionScorer

**Files:**
- Modify: `src/memory/dreaming/stages/consolidate.rs`
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Rewrite ConsolidateStage::execute**

```rust
// src/memory/dreaming/stages/consolidate.rs
//! ConsolidateStage: promotes STM facts to LTM using 8-dimensional scoring
//! and prunes weak facts.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::info;

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::consolidation::promotion_scorer::{
    compute_cluster_cohesion, compute_conceptual, compute_consolidation,
    compute_diversity, compute_frequency, compute_graph_centrality,
    compute_recency, compute_relevance, PromotionScorer,
};
use crate::memory::context::MemoryTier;
use crate::memory::dreaming::{should_prune, ConsolidationPipelineConfig};
use crate::memory::store::MemoryStore;

pub struct ConsolidateStage;

#[async_trait]
impl DreamStage for ConsolidateStage {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = ConsolidationPipelineConfig::default();
        let scorer = PromotionScorer::default();
        let now = chrono::Utc::now().timestamp();

        // 1. Get all valid facts
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let facts: Vec<_> = all_facts
            .into_iter()
            .take(config.max_facts_per_run)
            .collect();

        // 2. Collect ShortTerm fact IDs for signal aggregation
        let stm_facts: Vec<_> = facts.iter()
            .filter(|f| f.tier == MemoryTier::ShortTerm)
            .filter(|f| {
                let age_hours = (now - f.created_at).max(0) as u64 / 3600;
                age_hours >= scorer.thresholds.min_age_hours
            })
            .collect();

        let stm_ids: Vec<String> = stm_facts.iter().map(|f| f.id.clone()).collect();

        // 3. Batch query recall signal aggregates
        let signal_map: HashMap<String, _> = if !stm_ids.is_empty() {
            ctx.database.aggregate_for_facts(&stm_ids)?
                .into_iter()
                .map(|a| (a.fact_id.clone(), a))
                .collect()
        } else {
            HashMap::new()
        };

        // 4. Extract entity names from STM facts for graph centrality
        let entity_names: Vec<String> = stm_facts.iter()
            .flat_map(|f| extract_entity_names(&f.content))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let edge_counts = if !entity_names.is_empty() {
            ctx.graph_store.edge_count_for_entities(&entity_names, "default")?
        } else {
            HashMap::new()
        };

        // 5. Build cluster lookup from ctx.clusters
        let cluster_lookup = build_cluster_lookup(&ctx.clusters);

        // 6. Score and promote
        let mut consolidated_count = 0usize;
        let mut pruned_count = 0usize;

        for mut fact in facts {
            // Pruning (unchanged)
            if should_prune(&fact, config.pruning_threshold) {
                ctx.database
                    .invalidate_fact(&fact.id, "strength below pruning threshold")
                    .await?;
                pruned_count += 1;
                continue;
            }

            // Only score ShortTerm facts for promotion
            if fact.tier != MemoryTier::ShortTerm {
                continue;
            }

            let age_hours = (now - fact.created_at).max(0) as u64 / 3600;
            if age_hours < scorer.thresholds.min_age_hours {
                continue;
            }

            // Compute 8 dimensions
            let signal = signal_map.get(&fact.id);
            let signal_count = signal.map_or(0, |s| s.signal_count);
            let unique_queries = signal.map_or(0, |s| s.unique_queries);

            let dims = [
                compute_frequency(signal_count),
                compute_relevance(
                    signal.map_or(0.0, |s| s.total_score),
                    signal_count,
                ),
                compute_diversity(unique_queries, signal.map_or(0, |s| s.unique_channels)),
                compute_recency(signal.map_or(
                    (now - fact.updated_at) as f32 / 86400.0,
                    |s| (now - s.last_recall) as f32 / 86400.0,
                )),
                compute_consolidation(
                    signal.map_or(0, |s| s.recall_days),
                    signal.map_or(0.0, |s| (s.last_recall - s.first_recall) as f32 / 86400.0),
                ),
                compute_conceptual(count_conceptual_tags(&fact)),
                compute_graph_centrality(
                    max_entity_edges(&fact.content, &edge_counts),
                ),
                cluster_lookup.get(&fact.id).map_or(0.0, |(dist, radius)| {
                    compute_cluster_cohesion(*dist, *radius)
                }),
            ];

            let score = scorer.score(&dims);

            if scorer.should_promote(score, signal_count, unique_queries, age_hours) {
                fact.tier = MemoryTier::LongTerm;
                ctx.database.update_fact(&fact).await?;
                consolidated_count += 1;
            }
        }

        info!(
            consolidated = consolidated_count,
            pruned = pruned_count,
            "ConsolidateStage: 8-dim scoring complete"
        );

        Ok(ctx)
    }
}

/// Extract entity names from fact content (simple heuristic).
fn extract_entity_names(content: &str) -> Vec<String> {
    // Extract capitalized words as potential entity names
    content.split_whitespace()
        .filter(|w| w.len() > 1 && w.chars().next().map_or(false, |c| c.is_uppercase()))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Count conceptual tags from fact type and path.
fn count_conceptual_tags(fact: &crate::memory::context::MemoryFact) -> u32 {
    let mut count = 1u32; // fact_type itself counts as 1
    if let Some(ref path) = fact.path {
        count += path.split('/').filter(|s| !s.is_empty()).count() as u32;
    }
    count
}

/// Get max edge count for entities mentioned in content.
fn max_entity_edges(content: &str, edge_counts: &HashMap<String, u32>) -> u32 {
    extract_entity_names(content)
        .iter()
        .filter_map(|name| edge_counts.get(name))
        .copied()
        .max()
        .unwrap_or(0)
}

/// Build fact_id → (distance_to_centroid, cluster_radius) lookup from clusters.
fn build_cluster_lookup(
    clusters: &[crate::memory::dreaming::stages::cluster::MemoryCluster],
) -> HashMap<String, (f32, f32)> {
    let mut lookup = HashMap::new();
    for cluster in clusters {
        let radius = cluster.radius();
        for member in &cluster.members {
            lookup.insert(
                member.fact_id.clone(),
                (member.distance_to_centroid, radius),
            );
        }
    }
    lookup
}
```

- [ ] **Step 2: Remove should_consolidate from dreaming/mod.rs**

In `src/memory/dreaming/mod.rs`, delete the `should_consolidate` function (lines 584-588). Keep `should_prune` — it's still used.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib consolidate`
Expected: PASS

Run: `cargo test -p alephcore --lib dreaming`
Expected: PASS (no other code references `should_consolidate` in test paths)

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/consolidate.rs src/memory/dreaming/mod.rs
git commit -m "memory: rewrite ConsolidateStage with 8-dimensional promotion scoring"
```

---

### Task 6: DreamReportStore — Audit System

**Files:**
- Create: `src/memory/store/sqlite/dream_reports.rs`
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/mod.rs`

- [ ] **Step 1: Add DDL to schema.rs**

```rust
const DREAM_REPORTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_reports (
    id                    TEXT PRIMARY KEY,
    pipeline_type         TEXT NOT NULL,
    started_at            INTEGER NOT NULL,
    finished_at           INTEGER NOT NULL,
    duration_ms           INTEGER NOT NULL,
    facts_collected       INTEGER NOT NULL DEFAULT 0,
    clusters_found        INTEGER NOT NULL DEFAULT 0,
    drift_detected        INTEGER NOT NULL DEFAULT 0,
    drift_summary         TEXT,
    candidates_evaluated  INTEGER NOT NULL DEFAULT 0,
    facts_promoted        INTEGER NOT NULL DEFAULT 0,
    promotion_details     TEXT,
    facts_decayed         INTEGER NOT NULL DEFAULT 0,
    facts_pruned          INTEGER NOT NULL DEFAULT 0,
    nodes_decayed         INTEGER NOT NULL DEFAULT 0,
    edges_decayed         INTEGER NOT NULL DEFAULT 0,
    synthesis_count       INTEGER NOT NULL DEFAULT 0,
    errors                TEXT,
    namespace             TEXT NOT NULL DEFAULT 'owner'
);

CREATE INDEX IF NOT EXISTS idx_dream_reports_started
    ON dream_reports(started_at);
"#;
```

Add `conn.execute_batch(DREAM_REPORTS_DDL)` to `init_schema()`.

- [ ] **Step 2: Create dream_reports.rs**

```rust
// src/memory/store/sqlite/dream_reports.rs
//! DreamReportStore: persists dream pipeline execution reports for audit.

use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AlephError;
use super::SqliteMemoryBackend;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDreamReport {
    pub id: String,
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub facts_collected: u32,
    pub clusters_found: u32,
    pub drift_detected: bool,
    pub drift_summary: Option<String>,
    pub candidates_evaluated: u32,
    pub facts_promoted: u32,
    pub promotion_details: Option<String>,
    pub facts_decayed: u32,
    pub facts_pruned: u32,
    pub nodes_decayed: u32,
    pub edges_decayed: u32,
    pub synthesis_count: u32,
    pub errors: Option<String>,
    pub namespace: String,
}

impl SqliteMemoryBackend {
    /// Insert a dream report.
    pub fn insert_dream_report(&self, report: &PersistedDreamReport) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        conn.execute(
            "INSERT INTO dream_reports (id, pipeline_type, started_at, finished_at, duration_ms,
             facts_collected, clusters_found, drift_detected, drift_summary,
             candidates_evaluated, facts_promoted, promotion_details,
             facts_decayed, facts_pruned, nodes_decayed, edges_decayed,
             synthesis_count, errors, namespace)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                report.id, report.pipeline_type, report.started_at, report.finished_at,
                report.duration_ms, report.facts_collected, report.clusters_found,
                report.drift_detected as i32, report.drift_summary,
                report.candidates_evaluated, report.facts_promoted, report.promotion_details,
                report.facts_decayed, report.facts_pruned, report.nodes_decayed, report.edges_decayed,
                report.synthesis_count, report.errors, report.namespace,
            ],
        ).map_err(|e| AlephError::internal(format!("insert dream_report: {e}")))?;

        Ok(())
    }

    /// Query recent dream reports.
    pub fn recent_dream_reports(&self, limit: usize) -> Result<Vec<PersistedDreamReport>, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let mut stmt = conn.prepare(
            "SELECT id, pipeline_type, started_at, finished_at, duration_ms,
                    facts_collected, clusters_found, drift_detected, drift_summary,
                    candidates_evaluated, facts_promoted, promotion_details,
                    facts_decayed, facts_pruned, nodes_decayed, edges_decayed,
                    synthesis_count, errors, namespace
             FROM dream_reports ORDER BY started_at DESC LIMIT ?1"
        ).map_err(|e| AlephError::internal(format!("prepare recent_reports: {e}")))?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PersistedDreamReport {
                id: row.get(0)?,
                pipeline_type: row.get(1)?,
                started_at: row.get(2)?,
                finished_at: row.get(3)?,
                duration_ms: row.get(4)?,
                facts_collected: row.get(5)?,
                clusters_found: row.get(6)?,
                drift_detected: row.get::<_, i32>(7)? != 0,
                drift_summary: row.get(8)?,
                candidates_evaluated: row.get(9)?,
                facts_promoted: row.get(10)?,
                promotion_details: row.get(11)?,
                facts_decayed: row.get(12)?,
                facts_pruned: row.get(13)?,
                nodes_decayed: row.get(14)?,
                edges_decayed: row.get(15)?,
                synthesis_count: row.get(16)?,
                errors: row.get(17)?,
                namespace: row.get(18)?,
            })
        }).map_err(|e| AlephError::internal(format!("query recent_reports: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::internal(format!("read report row: {e}")))?);
        }
        Ok(results)
    }

    /// Get the latest dream report timestamp (for cache invalidation).
    pub fn latest_dream_report_ts(&self) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let ts = conn.query_row(
            "SELECT MAX(finished_at) FROM dream_reports",
            [],
            |row| row.get::<_, Option<i64>>(0),
        ).map_err(|e| AlephError::internal(format!("query latest ts: {e}")))?;

        Ok(ts)
    }
}
```

- [ ] **Step 3: Register module, write tests, run**

Add `pub mod dream_reports;` to `src/memory/store/sqlite/mod.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> crate::sync_primitives::Arc<SqliteMemoryBackend> {
        let tmp = tempfile::tempdir().expect("tempdir");
        crate::sync_primitives::Arc::new(SqliteMemoryBackend::new(tmp.path()).expect("backend"))
    }

    #[test]
    fn insert_and_query_report() {
        let db = setup();
        let report = PersistedDreamReport {
            id: "r1".into(),
            pipeline_type: "daily".into(),
            started_at: 1000,
            finished_at: 1060,
            duration_ms: 60000,
            facts_collected: 50,
            clusters_found: 3,
            drift_detected: false,
            drift_summary: None,
            candidates_evaluated: 10,
            facts_promoted: 2,
            promotion_details: None,
            facts_decayed: 5,
            facts_pruned: 1,
            nodes_decayed: 0,
            edges_decayed: 0,
            synthesis_count: 0,
            errors: None,
            namespace: "owner".into(),
        };
        db.insert_dream_report(&report).unwrap();

        let reports = db.recent_dream_reports(10).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].facts_promoted, 2);
    }

    #[test]
    fn latest_ts_empty() {
        let db = setup();
        assert_eq!(db.latest_dream_report_ts().unwrap(), None);
    }
}
```

Run: `cargo test -p alephcore --lib dream_reports`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/dream_reports.rs src/memory/store/sqlite/schema.rs src/memory/store/sqlite/mod.rs
git commit -m "memory: add DreamReportStore for dream pipeline audit"
```

---

### Task 7: Enhance DeepSynthesisStage

**Files:**
- Modify: `src/memory/dreaming/stages/synthesis.rs`

- [ ] **Step 1: Remove source fact specificity downgrade (Change 4)**

Delete lines 186-199 in the current `execute()` method — the block that sets `source_fact.specificity = FactSpecificity::Abstract`.

- [ ] **Step 2: Wire LLM synthesis (Change 1)**

Replace the naive content construction:

```rust
// Replace this:
let theme = format!("[{}] {}", fact_type_str, combined);
let content = format!("Pattern: {}", theme);

// With:
let content = if let Some(ref provider) = ctx.provider {
    let facts_tuples: Vec<(&str, f32, &str)> = cluster_facts.iter()
        .map(|f| (f.fact_type.as_str(), f.confidence, f.content.as_str()))
        .collect();
    let prompt = build_synthesis_prompt(&facts_tuples);
    match provider.complete_text(&prompt).await {
        Ok(response) => parse_synthesis_content(&response)
            .unwrap_or_else(|| format!("Pattern: {}", combined)),
        Err(e) => {
            warn!(error = %e, "LLM synthesis failed, using fallback");
            format!("Pattern: {}", combined)
        }
    }
} else {
    format!("Pattern: {}", combined)
};
```

Add helper:

```rust
fn parse_synthesis_content(response: &str) -> Option<String> {
    // Try to parse JSON response
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(response) {
        if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
            return Some(content.to_string());
        }
        if let Some(insight) = v.get("insight").and_then(|c| c.as_str()) {
            return Some(insight.to_string());
        }
    }
    // Fallback: use the raw response if it's reasonable length
    let trimmed = response.trim();
    if !trimmed.is_empty() && trimmed.len() < 500 {
        Some(trimmed.to_string())
    } else {
        None
    }
}
```

- [ ] **Step 3: Enable daily synthesis (Change 2)**

```rust
async fn should_run(&self, ctx: &DreamContext) -> bool {
    // Run on both daily and weekly, but with different data sources
    true
}
```

In `execute()`, add branch logic at the start:

```rust
let ltm_facts = if ctx.run_metadata.run_type == DreamRunType::Weekly {
    // Weekly: full LTM re-clustering (existing behavior)
    let all_facts = ctx.database.get_all_facts(false, None).await?;
    all_facts.into_iter()
        .filter(|f| f.tier == MemoryTier::LongTerm && f.is_valid)
        .collect()
} else {
    // Daily: use clusters from ConsolidateStage if available
    if ctx.clusters.is_empty() {
        debug!("DeepSynthesisStage: no clusters for daily synthesis");
        return Ok(ctx);
    }
    // Extract facts from clusters
    // ... collect cluster member facts
};
```

- [ ] **Step 4: Add dedup with refresh (Change 3)**

Before the fact insertion loop, fetch existing synthesis facts:

```rust
let existing_synthesis = ctx.database.get_all_facts(false, None).await?
    .into_iter()
    .filter(|f| f.fact_source == FactSource::Synthesis && f.is_valid)
    .collect::<Vec<_>>();

// In the insertion loop, before insert:
if let Some(ref new_emb) = synthesized.embedding {
    let near_dup = existing_synthesis.iter().find(|e| {
        e.embedding.as_ref().map_or(false, |emb| {
            cosine_similarity(new_emb, emb) > 0.85
        })
    });
    if let Some(dup) = near_dup {
        // Refresh timestamp to confirm still valid
        let mut refreshed = dup.clone();
        refreshed.updated_at = chrono::Utc::now().timestamp();
        if let Err(e) = ctx.database.update_fact(&refreshed).await {
            warn!(error = %e, "failed to refresh synthesis fact timestamp");
        }
        continue; // skip redundant insertion
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib synthesis`
Expected: PASS (existing tests should still pass — `should_run` change makes weekly test still pass, daily test now also returns true)

- [ ] **Step 6: Commit**

```bash
git add src/memory/dreaming/stages/synthesis.rs
git commit -m "memory: enhance DeepSynthesisStage with LLM synthesis, daily runs, dedup"
```

---

### Task 8: top_synthesized Query for Layered Retrieval

**Files:**
- Modify: `src/memory/store/sqlite/facts.rs`

- [ ] **Step 1: Add top_synthesized query**

```rust
impl SqliteMemoryBackend {
    /// Get top Synthesis facts ordered by updated_at descending (for Layer 1 background).
    pub async fn top_synthesized(
        &self,
        namespace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let mut stmt = conn.prepare(
            "SELECT * FROM facts
             WHERE fact_source = 'synthesis'
               AND is_valid = 1
               AND (namespace = ?1 OR namespace = 'owner')
             ORDER BY updated_at DESC
             LIMIT ?2"
        ).map_err(|e| AlephError::internal(format!("prepare top_synthesized: {e}")))?;

        let rows = stmt.query_map(params![namespace, limit as i64], |row| {
            Self::row_to_fact(row)
        }).map_err(|e| AlephError::internal(format!("query top_synthesized: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::internal(format!("read fact row: {e}")))?);
        }
        Ok(results)
    }

    /// Touch updated_at timestamp for a fact (for dedup refresh).
    pub async fn touch_fact_updated_at(&self, fact_id: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().map_err(|e| {
            AlephError::internal(format!("Failed to lock connection: {e}"))
        })?;

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE facts SET updated_at = ?1 WHERE id = ?2",
            params![now, fact_id],
        ).map_err(|e| AlephError::internal(format!("touch updated_at: {e}")))?;

        Ok(())
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib facts`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/memory/store/sqlite/facts.rs
git commit -m "memory: add top_synthesized query and touch_fact_updated_at for layered retrieval"
```

---

### Task 9: Layered MemoryContext Assembly

**Files:**
- Modify: `src/memory/fact_retrieval.rs`

- [ ] **Step 1: Add MemoryContext struct and build_layered_context**

```rust
/// Two-layer memory context for prompt injection.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 1: stable background (synthesis facts)
    pub background: Vec<MemoryFact>,
    /// Layer 2: query-relevant detail (hybrid search results)
    pub relevant: Vec<MemoryFact>,
}

impl MemoryContext {
    pub fn is_empty(&self) -> bool {
        self.background.is_empty() && self.relevant.is_empty()
    }

    /// Format for system prompt injection.
    pub fn to_prompt_sections(&self) -> String {
        let mut sections = String::new();

        if !self.background.is_empty() {
            sections.push_str("## Long-term knowledge about the user\n");
            for fact in &self.background {
                sections.push_str("- ");
                sections.push_str(&fact.content);
                sections.push('\n');
            }
            sections.push('\n');
        }

        if !self.relevant.is_empty() {
            sections.push_str("## Relevant memories for this conversation\n");
            for fact in &self.relevant {
                sections.push_str("- ");
                sections.push_str(&fact.content);
                sections.push('\n');
            }
        }

        sections
    }
}

impl FactRetrieval {
    /// Build a two-layer memory context.
    pub async fn build_layered_context(
        &self,
        query: &str,
        namespace: &NamespaceScope,
        channel: &str,
        session_id: Option<&str>,
    ) -> Result<MemoryContext, AlephError> {
        // Layer 1: background synthesis facts
        let background = self.database
            .top_synthesized(&namespace.effective_namespace(), 10)
            .await?;

        // Layer 2: query-relevant retrieval (with signal recording)
        let retrieval = self.retrieve_with_signals(query, namespace, channel, session_id).await?;

        Ok(MemoryContext {
            background,
            relevant: retrieval.facts,
        })
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib fact_retrieval`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/memory/fact_retrieval.rs
git commit -m "memory: add layered MemoryContext with background synthesis + query-relevant detail"
```

---

### Task 10: Wire Dream Reports into Pipeline

**Files:**
- Modify: `src/memory/dreaming/mod.rs`

- [ ] **Step 1: Add report persistence after pipeline run**

In the `DreamPipeline::run()` method (or in the DreamDaemon after pipeline completes), add report persistence:

```rust
use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;

// After pipeline.run() returns a DreamReport:
let persisted = PersistedDreamReport {
    id: uuid::Uuid::new_v4().to_string(),
    pipeline_type: match report.run_type {
        DreamRunType::Daily => "daily".into(),
        DreamRunType::Weekly => "weekly".into(),
    },
    started_at: run_start_ts,
    finished_at: chrono::Utc::now().timestamp(),
    duration_ms: elapsed.as_millis() as i64,
    facts_collected: report.memory_count as u32,
    clusters_found: report.clusters_count as u32,
    drift_detected: report.drift_resolutions_count > 0,
    drift_summary: None,
    candidates_evaluated: 0, // TODO: pass through from ConsolidateStage
    facts_promoted: report.new_facts_count as u32,
    promotion_details: None,
    facts_decayed: report.memory_decay_report.as_ref().map_or(0, |r| r.decayed_count as u32),
    facts_pruned: report.memory_decay_report.as_ref().map_or(0, |r| r.pruned_count as u32),
    nodes_decayed: report.graph_decay_report.as_ref().map_or(0, |r| r.nodes_decayed as u32),
    edges_decayed: report.graph_decay_report.as_ref().map_or(0, |r| r.edges_decayed as u32),
    synthesis_count: report.synthesis_insights_count as u32,
    errors: None,
    namespace: "owner".into(),
};

if let Err(e) = database.insert_dream_report(&persisted) {
    tracing::warn!("Failed to persist dream report: {e}");
}
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib dreaming`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/memory/dreaming/mod.rs
git commit -m "memory: persist dream pipeline reports for audit"
```

---

### Task 11: Cleanup Old Consolidation Code

**Files:**
- Modify: `src/memory/dreaming/mod.rs`
- Modify: `src/memory/consolidation/analyzer.rs`

- [ ] **Step 1: Remove should_consolidate export**

In `src/memory/dreaming/mod.rs`, remove `should_consolidate` from pub exports if still listed. The function was already deleted in Task 5. Verify no remaining references.

- [ ] **Step 2: Clean up analyzer.rs**

In `src/memory/consolidation/analyzer.rs`, the `ConsolidationAnalyzer` still serves a purpose (profile generation). No deletion needed here — it's independent from the dreaming consolidation. But verify that `calculate_frequency_scores` doesn't reference `should_consolidate`.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore`
Expected: PASS — no broken references

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "memory: clean up old consolidation logic, remove dead references"
```

---

### Task 12: Integration Verification

**Files:** None (verification only)

- [ ] **Step 1: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors, no warnings about unused imports

- [ ] **Step 2: Run all memory tests**

Run: `cargo test -p alephcore --lib memory`
Expected: All PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Verify schema initialization**

Run a quick manual test: start the server and check that `recall_signals` and `dream_reports` tables exist in the SQLite database.

```bash
sqlite3 ~/.aleph/data/memory.db ".tables" | grep -E "recall_signals|dream_reports"
```
Expected: Both tables listed

- [ ] **Step 5: Final commit if any clippy fixes**

```bash
git add -A
git commit -m "memory: fix clippy warnings from dream consolidation enhancement"
```

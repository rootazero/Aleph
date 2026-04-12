# Rerank + Query Expander Wiring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the existing `rerank/` and `query_expander` modules into the production retrieval path via `FactRetrieval`.

**Architecture:** Both modules integrate into a single method — `FactRetrieval::hybrid_search_with_fallback_limit()`. Query expansion runs before the hybrid search call (expanding BM25 query text). Rerank runs after the scoring pipeline (re-scoring with a cross-encoder). Both degrade gracefully on failure.

**Tech Stack:** Rust, async_trait, reqwest (rerank HTTP calls), serde_json

---

## Task 1: Integrate query_expander into FactRetrieval

**Files:**
- Modify: `src/memory/fact_retrieval.rs:212-246` — call `query_expander::expand()` at the top of `hybrid_search_with_fallback_limit`

- [ ] **Step 1: Add import**

At the top of `src/memory/fact_retrieval.rs`, add:

```rust
use crate::memory::query_expander;
```

- [ ] **Step 2: Apply expansion in hybrid_search_with_fallback_limit**

In `hybrid_search_with_fallback_limit` (line 212), add query expansion as the first operation. Change the method from:

```rust
    async fn hybrid_search_with_fallback_limit(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<RetrievalResult, AlephError> {
        let dim_hint = query_embedding.len() as u32;

        // Try hybrid search first (vector + BM25 RRF fusion)
        let scored_facts = match self
            .database
            .hybrid_search(&HybridSearchParams {
                embedding: query_embedding,
                dim_hint,
                query_text,
                vector_weight: self.hybrid_config.vector_weight,
                text_weight: self.hybrid_config.text_weight,
                filter,
                limit,
            })
            .await
```

To:

```rust
    async fn hybrid_search_with_fallback_limit(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<RetrievalResult, AlephError> {
        let dim_hint = query_embedding.len() as u32;

        // Expand query for BM25 (appends Chinese synonyms for CJK queries)
        let expanded = query_expander::expand(query_text);

        // Try hybrid search first (vector + BM25 RRF fusion)
        let scored_facts = match self
            .database
            .hybrid_search(&HybridSearchParams {
                embedding: query_embedding,
                dim_hint,
                query_text: &expanded.bm25_query,
                vector_weight: self.hybrid_config.vector_weight,
                text_weight: self.hybrid_config.text_weight,
                filter,
                limit,
            })
            .await
```

The only change: `query_text` → `&expanded.bm25_query` in the `HybridSearchParams`. The vector embedding is already computed from the original query (upstream), so semantic search is unaffected.

- [ ] **Step 3: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -10
```

Expected: clean compilation. `expand()` returns `ExpandedQuery` with `bm25_query: String`, and `HybridSearchParams.query_text` is `&str` — the borrow works.

- [ ] **Step 4: Run existing tests**

```bash
cargo test -p alephcore --lib memory::query_expander -- -v 2>&1 | tail -10
cargo test -p alephcore --lib memory::fact_retrieval -- -v 2>&1 | tail -10
```

Expected: all pass (query_expander 4 tests, fact_retrieval tests).

- [ ] **Step 5: Commit**

```bash
git add src/memory/fact_retrieval.rs
git commit -m "memory: integrate query_expander into hybrid search path

BM25 text queries now pass through query_expander::expand() before
hybrid search. CJK queries get Chinese synonyms appended for better
keyword recall. Non-CJK queries are unchanged (zero-cost passthrough)."
```

---

## Task 2: Add RerankConfig to FactRetrieval

**Files:**
- Modify: `src/memory/fact_retrieval.rs` — add `rerank_config` field, update constructors

- [ ] **Step 1: Add rerank_config field to FactRetrieval struct**

In `src/memory/fact_retrieval.rs`, add the import and field:

```rust
use crate::memory::rerank::RerankConfig;
```

Change the struct:

```rust
pub struct FactRetrieval {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: FactRetrievalConfig,
    hybrid_config: HybridSearchConfig,
    scoring_pipeline: ScoringPipeline,
    scoring_config: ScoringPipelineConfig,
    rerank_config: RerankConfig,
}
```

- [ ] **Step 2: Update constructors**

In `new()`, add `rerank_config: RerankConfig::default()` to the struct initialization:

```rust
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: FactRetrievalConfig,
    ) -> Self {
        let scoring_config = ScoringPipelineConfig::default();
        let scoring_pipeline = ScoringPipeline::from_config(&scoring_config);
        Self {
            database,
            embedder,
            config,
            hybrid_config: HybridSearchConfig::default(),
            scoring_pipeline,
            scoring_config,
            rerank_config: RerankConfig::default(),
        }
    }
```

Add a new builder method:

```rust
    /// Set the rerank configuration (enables cross-encoder reranking when config.enabled = true)
    pub fn with_rerank_config(mut self, config: RerankConfig) -> Self {
        self.rerank_config = config;
        self
    }
```

- [ ] **Step 3: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 4: Commit**

```bash
git add src/memory/fact_retrieval.rs
git commit -m "memory: add RerankConfig field to FactRetrieval

Adds rerank_config field (default: disabled) and with_rerank_config()
builder method. No behavioral change yet — rerank integration in next
commit."
```

---

## Task 3: Implement rerank integration in hybrid_search_with_fallback_limit

**Files:**
- Modify: `src/memory/fact_retrieval.rs` — add `apply_rerank` method, call it after scoring pipeline

- [ ] **Step 1: Add rerank imports**

At the top of `src/memory/fact_retrieval.rs`, add:

```rust
use crate::memory::rerank::{build_provider, blend_scores};
```

- [ ] **Step 2: Add apply_rerank private method**

Add this method to the `impl FactRetrieval` block (after `apply_pipeline`):

```rust
    /// Apply cross-encoder reranking to scored results.
    ///
    /// Calls the configured rerank provider (HTTP API), then blends the
    /// cross-encoder scores with the original retrieval scores.
    /// Returns the original results unchanged on any error (graceful fallback).
    async fn apply_rerank(
        &self,
        query: &str,
        results: Vec<ScoredFact>,
    ) -> Vec<ScoredFact> {
        if results.is_empty() {
            return results;
        }

        let provider = build_provider(&self.rerank_config);
        let documents: Vec<String> = results.iter().map(|sf| sf.fact.content.clone()).collect();
        let top_n = results.len();

        let rerank_results = match provider.rerank(query, &documents, top_n).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "Rerank failed, returning un-reranked results");
                return results;
            }
        };

        // Build (id, original_score) pairs for blend_scores
        let originals: Vec<(String, f32)> = results
            .iter()
            .map(|sf| (sf.fact.id.clone(), sf.score))
            .collect();

        let blended = blend_scores(
            &originals,
            &rerank_results,
            self.rerank_config.rerank_weight,
        );

        // Reconstruct ScoredFact vec in blended order
        let mut reranked = Vec::with_capacity(blended.len());
        for (id, score) in &blended {
            if let Some(sf) = results.iter().find(|sf| &sf.fact.id == id) {
                reranked.push(ScoredFact {
                    fact: sf.fact.clone(),
                    score: *score,
                });
            }
        }
        reranked
    }
```

- [ ] **Step 3: Wire apply_rerank into hybrid_search_with_fallback_limit**

In `hybrid_search_with_fallback_limit`, after the `apply_pipeline` call (currently line 249) and before the threshold filter (currently line 252), add the rerank step:

Change from:

```rust
        // Apply scoring pipeline for re-ranking
        let scored_facts = self.apply_pipeline(scored_facts, query_embedding, query_text);

        // Filter by threshold and convert
        let facts: Vec<MemoryFact> = scored_facts
```

To:

```rust
        // Apply scoring pipeline for re-ranking
        let scored_facts = self.apply_pipeline(scored_facts, query_embedding, query_text);

        // Apply cross-encoder reranking if enabled
        let scored_facts = if self.rerank_config.enabled {
            self.apply_rerank(query_text, scored_facts).await
        } else {
            scored_facts
        };

        // Filter by threshold and convert
        let facts: Vec<MemoryFact> = scored_facts
```

- [ ] **Step 4: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 5: Run all memory tests**

```bash
cargo test -p alephcore --lib memory::fact_retrieval -- -v 2>&1 | tail -10
cargo test -p alephcore --lib memory::rerank -- -v 2>&1 | tail -10
```

Expected: all pass. Rerank is disabled by default (`RerankConfig::default().enabled == false`), so existing tests are unaffected.

- [ ] **Step 6: Commit**

```bash
git add src/memory/fact_retrieval.rs
git commit -m "memory: integrate cross-encoder reranking into retrieval path

When config.rerank.enabled is true, retrieval results are re-scored by
a cross-encoder provider (Jina/SiliconFlow/Voyage/Pinecone/vLLM) after
the scoring pipeline. Scores are blended with configurable weight
(default 0.6 rerank + 0.4 original). Graceful fallback on HTTP errors."
```

---

## Task 4: Wire RerankConfig from MemoryConfig to FactRetrieval construction sites

**Files:**
- Modify: callers that construct `FactRetrieval` — search for `FactRetrieval::new` and `FactRetrieval::with_defaults`

- [ ] **Step 1: Find all FactRetrieval construction sites**

```bash
grep -rn "FactRetrieval::new\|FactRetrieval::with_defaults" src/ --include="*.rs"
```

- [ ] **Step 2: Update each construction site**

For each site where `FactRetrieval` is constructed and a `MemoryConfig` is available, chain `.with_rerank_config(config.rerank.clone())`:

Example pattern:
```rust
// Before:
let retrieval = FactRetrieval::with_defaults(database.clone(), embedder.clone());

// After:
let retrieval = FactRetrieval::with_defaults(database.clone(), embedder.clone())
    .with_rerank_config(memory_config.rerank.clone());
```

For sites where `MemoryConfig` is NOT available (e.g., test helpers, standalone usage), leave as-is — `RerankConfig::default()` has `enabled: false`, so rerank is a no-op.

- [ ] **Step 3: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib memory:: -- --test-threads=1 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "memory: pass RerankConfig from MemoryConfig to FactRetrieval at construction

All production FactRetrieval construction sites now receive the user's
rerank configuration. Test construction sites use the default (disabled)
config."
```

---

## Task 5: Final Verification

- [ ] **Step 1: Full compilation**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 2: Run all memory tests**

```bash
cargo test -p alephcore --lib memory:: -- --test-threads=1 2>&1 | tail -10
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | grep "error\[" | head -10
```

Expected: zero errors.

- [ ] **Step 4: Verify data flow end-to-end**

Confirm the complete retrieval path:
```bash
grep -n "query_expander::expand\|apply_rerank\|rerank_config" src/memory/fact_retrieval.rs
```

Expected output shows:
1. `expand()` call in `hybrid_search_with_fallback_limit`
2. `apply_rerank()` call after scoring pipeline
3. `rerank_config` field and builder method

- [ ] **Step 5: Commit if any clippy fixes needed**

```bash
git add -A
git commit -m "memory: fix clippy warnings after rerank + query_expander wiring"
```

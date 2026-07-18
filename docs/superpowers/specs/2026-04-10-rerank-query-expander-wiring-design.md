# Rerank + Query Expander Wiring

> Wire the existing rerank and query_expander modules into the production retrieval path.

**Date**: 2026-04-10
**Scope**: `src/memory/fact_retrieval.rs` integration, `src/memory/rerank/`, `src/memory/query_expander.rs`
**Prerequisite**: Memory Logic Chain Fix (completed — HybridRetrieval already wired)

---

## Problem

`rerank/` (5 cross-encoder providers) and `query_expander.rs` (Chinese synonym expansion) are fully implemented and tested but never called from any production path. This means:
- BM25 queries go un-expanded — Chinese synonyms miss matches that vector search catches
- Cross-encoder reranking is configured via `MemoryConfig.rerank` but never applied to results

## Solution

Integrate both modules into `FactRetrieval.hybrid_search_with_fallback_limit()` — the single method all production retrieval flows through after the recent HybridRetrieval wiring.

## Data Flow (After Wiring)

```
query input
  → query_expander::expand(query)
    → original (for vector_search embedding)
    → bm25_query (for text_search, with Chinese synonyms appended)
  → database.hybrid_search(embedding_of_original, bm25_query, ...)
  → ScoringPipeline.run() [7 stages]
  → if config.rerank.enabled && provider configured:
      → build_provider(&config.rerank)
      → provider.rerank(query, doc_contents, top_n)
      → blend_scores(pipeline_results, rerank_results, rerank_weight)
  → return final results
```

## Changes

### 1. Query Expansion Integration

**Location**: `fact_retrieval.rs`, at the entry of `hybrid_search_with_fallback_limit()`

```rust
use crate::memory::query_expander;

// Before embedding, expand query for BM25
let expanded = query_expander::expand(query_text);
// Use expanded.original for embedding (unchanged semantics)
// Use expanded.bm25_query for text_search parameter in hybrid_search
```

Currently `hybrid_search_with_fallback_limit` receives `query_text: &str` and passes it directly to `database.hybrid_search()`. After the change:
- The embedding is computed from `expanded.original` (same as before — the verbatim query)
- The BM25 text search uses `expanded.bm25_query` (with synonyms appended for CJK queries)
- For non-CJK queries, `expand()` returns identical original and bm25_query — zero behavioral change

### 2. Rerank Integration

**Location**: `fact_retrieval.rs`, after scoring pipeline execution, before returning results

```rust
use crate::memory::rerank::{build_provider, blend_scores};

// After scoring pipeline produces sorted results:
if self.config.rerank.enabled {
    match self.apply_rerank(query_text, &results).await {
        Ok(reranked) => results = reranked,
        Err(e) => {
            tracing::warn!(error = %e, "Rerank failed, returning un-reranked results");
            // Graceful fallback: return scoring pipeline results as-is
        }
    }
}
```

New private method on FactRetrieval:
```rust
async fn apply_rerank(
    &self,
    query: &str,
    results: &[ScoredFact],
) -> Result<Vec<ScoredFact>, AlephError> {
    let provider = build_provider(&self.config.rerank);
    let documents: Vec<String> = results.iter().map(|sf| sf.fact.content.clone()).collect();
    let top_n = results.len();

    let rerank_results = provider.rerank(query, &documents, top_n).await?;

    // Build (doc_id, original_score) pairs for blend_scores
    let originals: Vec<(String, f32)> = results
        .iter()
        .map(|sf| (sf.fact.id.clone(), sf.score))
        .collect();

    let blended = blend_scores(&originals, &rerank_results, self.config.rerank.rerank_weight);

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
    Ok(reranked)
}
```

### 3. Config Access

`FactRetrieval` already receives `Arc<MemoryConfig>` which contains `rerank: RerankConfig`. No new constructor parameters needed — just access `self.config.rerank`.

### 4. No Changes to Existing Modules

- `rerank/` — all 5 providers used as-is
- `query_expander.rs` — `expand()` function used as-is
- `scoring_pipeline/` — unchanged
- `hybrid_retrieval/` — unchanged

## Error Handling

| Failure | Behavior |
|---------|----------|
| query_expander returns unchanged query | No-op for non-CJK — by design |
| Rerank provider HTTP timeout | Log warning, return un-reranked results |
| Rerank provider returns error | Log warning, return un-reranked results |
| Rerank provider returns partial results | blend_scores handles missing indices (score=0.0) |
| config.rerank.enabled = false (default) | Skip rerank entirely — zero overhead |

## Testing

- Existing `rerank/` tests (5 unit tests) — unchanged
- Existing `query_expander` tests (4 unit tests) — unchanged  
- New: verify `expand()` is called before hybrid search (check that BM25 query differs from original for CJK input)
- Rerank integration test is impractical without network mock — rely on graceful fallback and existing provider unit tests

## Out of Scope

- Adding new rerank providers
- Modifying synonym tables in query_expander
- Changing ScoringPipeline stages
- Making query expansion configurable (it's always-on, zero-cost for non-CJK)

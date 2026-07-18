# Retrieval Trace Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the placeholder `memory.retrieve_with_trace` RPC with a real read-through over `NoteFactRetrieval`, so the existing Settings ▸ Memory "Retrieval Debug Panel" shows real scored results + real per-stage telemetry.

**Architecture:** Add an additive `NoteFactRetrieval::retrieve_traced()` that runs the exact same orchestration as `retrieve()` but records each stage's timing + working-set sizes via a `TraceSink` (the hot `retrieve()` path passes `TraceSink::Off`, a no-op). Move the handler from the dependency-free registry block into `register_memory_handlers` to receive `memory_db` + `embedder` + `app_config`, construct `NoteFactRetrieval` per the production recipe, and map results+stages to the panel's existing JSON contract. No frontend change.

**Tech Stack:** Rust (tokio, async_trait, serde_json), Aleph memory subsystem, JSON-RPC gateway.

## Global Constraints

- **Backend only.** No changes under `interfaces/webchat/`. The panel already consumes the contract.
- **No new dependencies.**
- **`cargo check` IS allowed this iteration** (user override for #3 to ensure safety). Prefer `cargo check -p alephcore` and `cargo check -p alephcore --bin aleph-server`; 极度节制 cargo 调用 — run checks only at the steps that say so, not after every edit. Do NOT run full `cargo test`/`clippy` suites unless a step says to.
- **Branch isolation.** All work in a NEW git worktree branch off `main` (`memory-retrieval-trace-wiring`); never edit `main` directly.
- **Entropy reduction.** Delete the placeholder handler registration after migrating.
- **Behavior preservation.** `retrieve()` results and ordering must stay byte-identical (trace is observational).
- Graph/note identity: the retrieval pipeline keys facts on `ScoredFact.fact.id` (set to the note path by `scored_fact_from_index_entry`). The trace `id` field uses `fact.id`.
- Reply language Chinese; code comments English.

---

### Task 0: Create the worktree branch

- [ ] **Step 1: Create an isolated worktree off main**

Use the `superpowers:using-git-worktrees` skill (or `EnterWorktree`) to create a worktree on a new branch `memory-retrieval-trace-wiring` based on the current local `main` HEAD. (Note: local `main` is ahead of `origin/main`; branch off local `main`, not `origin/main`.) All subsequent edits happen inside that worktree. Do not touch `main`.

---

### Task 1: Trace types (`StageTrace` + `TraceSink`)

**Files:**
- Create: `src/memory/note_retrieval/trace.rs`
- Modify: `src/memory/note_retrieval/mod.rs` (add `pub mod trace;` + a `use`)

**Interfaces:**
- Produces: `pub struct StageTrace { pub name: String, pub duration_ms: u64, pub input_count: usize, pub output_count: usize }` (derives `Debug, Clone, PartialEq`); `pub enum TraceSink { Off, On(Vec<StageTrace>) }` with `pub fn record(&mut self, name: &str, duration_ms: u64, input_count: usize, output_count: usize)` and `pub fn into_stages(self) -> Vec<StageTrace>`.

- [ ] **Step 1: Create `src/memory/note_retrieval/trace.rs`**

```rust
//! Inline scoring-pipeline telemetry for `NoteFactRetrieval::retrieve_traced`.
//! Observational only: the hot `retrieve()` path uses `TraceSink::Off`, whose
//! `record` is a no-op and allocates nothing.

/// One scoring-pipeline stage's measured telemetry.
#[derive(Debug, Clone, PartialEq)]
pub struct StageTrace {
    /// Stage name, e.g. "hybrid_search", "rerank", "truncate".
    pub name: String,
    /// Wall-clock time spent in the stage.
    pub duration_ms: u64,
    /// Working-set size entering the stage.
    pub input_count: usize,
    /// Working-set size leaving the stage.
    pub output_count: usize,
}

/// Collects per-stage telemetry only in traced mode. `Off` is the hot path:
/// `record` is a no-op and no `Vec` is allocated.
pub enum TraceSink {
    Off,
    On(Vec<StageTrace>),
}

impl TraceSink {
    /// Record one stage. No-op when `Off`.
    pub fn record(&mut self, name: &str, duration_ms: u64, input_count: usize, output_count: usize) {
        if let Self::On(stages) = self {
            stages.push(StageTrace {
                name: name.to_string(),
                duration_ms,
                input_count,
                output_count,
            });
        }
    }

    /// Consume the sink, returning collected stages (empty when `Off`).
    pub fn into_stages(self) -> Vec<StageTrace> {
        match self {
            Self::On(stages) => stages,
            Self::Off => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_sink_off_records_nothing() {
        let mut s = TraceSink::Off;
        s.record("x", 1, 2, 3);
        assert!(s.into_stages().is_empty());
    }

    #[test]
    fn trace_sink_on_collects_in_order() {
        let mut s = TraceSink::On(Vec::new());
        s.record("a", 1, 0, 5);
        s.record("b", 2, 5, 5);
        let stages = s.into_stages();
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].name, "a");
        assert_eq!(stages[0].output_count, 5);
        assert_eq!(stages[1].input_count, 5);
    }
}
```

- [ ] **Step 2: Register the module in `src/memory/note_retrieval/mod.rs`**

The current module declarations are (lines 7–9):
```rust
pub mod expansion;
pub mod hybrid;
pub mod scoring;
```
Add `trace` (keep alphabetical):
```rust
pub mod expansion;
pub mod hybrid;
pub mod scoring;
pub mod trace;
```

- [ ] **Step 3: Bring the types + `Instant` into scope in `mod.rs`**

Near the top of `mod.rs`, after the existing `use` block, add:
```rust
use self::trace::{StageTrace, TraceSink};
use std::time::Instant;
```
(If `std::time::Instant` is already imported, do not duplicate it. `StageTrace`/`TraceSink` are new.)

- [ ] **Step 4 (SKIP RUN — optional check): verify compile**

Optional: `cargo check -p alephcore`. Expected: clean (new `pub` items are reachable API, no dead-code warning). Per the resource constraint, you may defer this to Task 2's check.

- [ ] **Step 5: Commit**

```bash
git add src/memory/note_retrieval/trace.rs src/memory/note_retrieval/mod.rs
git commit -m "memory: add StageTrace + TraceSink for retrieval telemetry"
```

---

### Task 2: `retrieve_traced` + inline stage instrumentation

**Files:**
- Modify: `src/memory/note_retrieval/mod.rs` (refactor `retrieve`, add `retrieve_inner` + `retrieve_traced`, thread `sink` through `apply_rerank` + `apply_scoring`)

**Interfaces:**
- Consumes: `StageTrace`, `TraceSink` (Task 1).
- Produces: `pub async fn retrieve_traced(&self, query: &str, agent_id: &str, limit: usize) -> Result<(Vec<ScoredFact>, Vec<StageTrace>), AlephError>`. `retrieve()` keeps its existing public signature and behavior.
- Internal: `apply_rerank` and `apply_scoring` gain a trailing `sink: &mut TraceSink` parameter.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` in `mod.rs` (it already has `create_retrieval()`, `scored()`, `inactive_scoring()` helpers and imports `super::*`):

```rust
    /// Active scoring config so apply_scoring exercises all three sub-stages.
    fn active_scoring() -> RetrievalScoringConfig {
        RetrievalScoringConfig {
            recency_enabled: true,
            reinforcement_enabled: true,
            mmr_enabled: true,
            ..RetrievalScoringConfig::default()
        }
    }

    #[tokio::test]
    async fn apply_scoring_trace_matches_untraced_and_records_stages() {
        let (retrieval, _dir) = create_retrieval().await;
        let retrieval = retrieval.with_scoring_config(&active_scoring());

        let facts = vec![
            scored("a", "alpha content one", 0.9),
            scored("b", "beta content two", 0.5),
            scored("c", "gamma content three", 0.3),
        ];
        let counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

        // Untraced (Off) reference result.
        let mut off = TraceSink::Off;
        let ref_out = retrieval.apply_scoring(facts.clone(), 1_700_000_000, &counts, &mut off);

        // Traced (On) result must be identical in scores + order.
        let mut on = TraceSink::On(Vec::new());
        let traced_out = retrieval.apply_scoring(facts, 1_700_000_000, &counts, &mut on);

        let ref_ids: Vec<(&str, f32)> =
            ref_out.iter().map(|f| (f.fact.id.as_str(), f.score)).collect();
        let traced_ids: Vec<(&str, f32)> =
            traced_out.iter().map(|f| (f.fact.id.as_str(), f.score)).collect();
        assert_eq!(ref_ids, traced_ids, "tracing must not change results");

        let stages = on.into_stages();
        let names: Vec<&str> = stages.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"recency_decay"));
        assert!(names.contains(&"reinforcement"));
        assert!(names.contains(&"mmr_diversity"));
        // Recency/reinforcement preserve cardinality.
        for s in &stages {
            if s.name == "recency_decay" || s.name == "reinforcement" {
                assert_eq!(s.input_count, s.output_count);
            }
        }
    }

    #[tokio::test]
    async fn retrieve_traced_on_empty_store_returns_stages() {
        let (retrieval, _dir) = create_retrieval().await;
        let (results, stages) = retrieval.retrieve_traced("anything", "main", 5).await.unwrap();
        assert!(results.is_empty(), "empty store yields no results");
        // The search stage always runs; with a mock embedder it is hybrid_search.
        assert!(
            stages.iter().any(|s| s.name == "hybrid_search" || s.name == "fts_search"),
            "a search stage must be recorded, got {stages:?}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail** — `cargo test -p alephcore note_retrieval::tests::apply_scoring_trace_matches_untraced_and_records_stages note_retrieval::tests::retrieve_traced_on_empty_store_returns_stages` — Expected: FAIL to compile (`apply_scoring` takes 3 args, `retrieve_traced` undefined). This is the RED state. (Allowed under the #3 cargo override; if conserving, you may rely on the compiler's signature errors being self-evident and proceed.)

- [ ] **Step 3: Replace `retrieve()` with the thin wrapper + `retrieve_inner` + `retrieve_traced`**

Replace the entire existing `retrieve()` method (current lines 263–319) with:

```rust
    /// Hybrid vector + FTS search with RRF fusion.
    /// Returns `ScoredFact` for downstream compatibility.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let mut sink = TraceSink::Off;
        self.retrieve_inner(query, agent_id, limit, &mut sink).await
    }

    /// Same as [`retrieve`], but also returns per-stage telemetry for the
    /// retrieval debug panel. Results and ordering are identical to `retrieve`.
    pub async fn retrieve_traced(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<ScoredFact>, Vec<StageTrace>), AlephError> {
        let mut sink = TraceSink::On(Vec::new());
        let results = self.retrieve_inner(query, agent_id, limit, &mut sink).await?;
        Ok((results, sink.into_stages()))
    }

    /// Shared orchestration for `retrieve` / `retrieve_traced`. The `sink`
    /// records stage telemetry only when `On`; `Off` is a no-op hot path with
    /// byte-identical results.
    async fn retrieve_inner(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        sink: &mut TraceSink,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // Embedding requires a remote API call; when that endpoint is
        // unreachable (network outage, provider down) the notes themselves
        // are still local — degrade to FTS-only search instead of failing.
        let embedding = match self.embedder.embed(query).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "note retrieval: embedding unavailable, falling back to FTS-only search"
                );
                let t0 = Instant::now();
                let results = self.text_retrieve(query, agent_id, limit).await?;
                sink.record("fts_search", t0.elapsed().as_millis() as u64, 0, results.len());
                return Ok(results);
            }
        };
        let dim = embedding.len() as u32;

        let t0 = Instant::now();
        let mut results = self
            .indexer
            .store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, self.fetch_limit(limit))
            .await?;
        sink.record("hybrid_search", t0.elapsed().as_millis() as u64, 0, results.len());

        if self.expansion.is_active() {
            let t0 = Instant::now();
            let before = results.len();
            let peers = expansion::graph_expand(
                self.indexer.store().as_ref(),
                agent_id,
                &results,
                &self.expansion,
            )
            .await;
            results.extend(peers);
            // Bound the merged pool so rerank cost stays capped despite expansion.
            if results.len() > RERANK_MAX_CANDIDATES {
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results.truncate(RERANK_MAX_CANDIDATES);
            }
            sink.record("graph_expand", t0.elapsed().as_millis() as u64, before, results.len());
        }

        let facts: Vec<ScoredFact> = results.iter().map(|r| r.to_scored_fact(agent_id)).collect();
        let ranked = self.apply_rerank(query, facts, sink).await;
        let counts = self.fetch_reinforcement_counts(&ranked).await;
        let mut ranked = self.apply_scoring(ranked, now_unix(), &counts, sink);
        let before = ranked.len();
        let t0 = Instant::now();
        ranked.truncate(limit);
        sink.record("truncate", t0.elapsed().as_millis() as u64, before, ranked.len());
        // Close the hot-floating loop: record the surfaced notes as recall hits.
        self.record_recall(query, agent_id, &ranked).await;
        Ok(ranked)
    }
```

- [ ] **Step 4: Thread `sink` into `apply_rerank`**

Replace the entire existing `apply_rerank` method (current lines 222–259) with (only the signature gains `sink` and one `sink.record` line is added before the final `out`):

```rust
    async fn apply_rerank(
        &self,
        query: &str,
        facts: Vec<ScoredFact>,
        sink: &mut TraceSink,
    ) -> Vec<ScoredFact> {
        let Some(reranker) = self.reranker.as_ref() else {
            return facts;
        };
        // Nothing to reorder for trivial sets.
        if facts.len() < 2 {
            return facts;
        }
        let t0 = Instant::now();
        let input = facts.len();

        let docs: Vec<String> = facts.iter().map(|f| f.fact.content.clone()).collect();
        let reranked = match reranker.rerank(query, &docs, docs.len()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    provider = reranker.provider_id(),
                    "cross-encoder rerank failed; keeping original order"
                );
                return facts;
            }
        };

        let originals: Vec<(String, f32)> =
            facts.iter().map(|f| (f.fact.id.clone(), f.score)).collect();
        let blended = blend_scores(&originals, &reranked, self.rerank_weight);

        // Rebuild ScoredFacts in blended order, carrying the new scores.
        let mut by_path: HashMap<String, ScoredFact> =
            facts.into_iter().map(|f| (f.fact.id.clone(), f)).collect();
        let mut out = Vec::with_capacity(by_path.len());
        for (path, score) in blended {
            if let Some(mut fact) = by_path.remove(&path) {
                fact.score = score;
                out.push(fact);
            }
        }
        sink.record("rerank", t0.elapsed().as_millis() as u64, input, out.len());
        out
    }
```

- [ ] **Step 5: Thread `sink` into `apply_scoring` (split the loop into per-stage passes)**

Replace the entire existing `apply_scoring` method (current lines 125–176) with the version below. The recency+reinforcement loop is split into two independent flag-guarded passes so each can be timed separately; this is **result-identical** because each adjustment is per-fact (`reinforcement(recency(score))` composes the same regardless of pass structure) and the single re-sort happens after both:

```rust
    fn apply_scoring(
        &self,
        facts: Vec<ScoredFact>,
        now: i64,
        reinforcement_counts: &HashMap<String, i64>,
        sink: &mut TraceSink,
    ) -> Vec<ScoredFact> {
        if !self.scoring.is_active() || facts.len() < 2 {
            return facts;
        }
        let mut facts = facts;

        // 1a) Recency reweight.
        if self.scoring.recency_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            for f in facts.iter_mut() {
                let mult = scoring::recency_multiplier(
                    f.fact.updated_at,
                    now,
                    self.scoring.recency_half_life_days,
                );
                f.score = scoring::apply_recency(f.score, mult, self.scoring.recency_weight);
            }
            sink.record("recency_decay", t0.elapsed().as_millis() as u64, n, n);
        }

        // 1b) Reinforcement reweight.
        if self.scoring.reinforcement_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            for f in facts.iter_mut() {
                let hits = reinforcement_counts.get(&f.fact.id).copied().unwrap_or(0);
                f.score =
                    scoring::apply_reinforcement(f.score, hits, self.scoring.reinforcement_weight);
            }
            sink.record("reinforcement", t0.elapsed().as_millis() as u64, n, n);
        }

        // 1c) Re-sort by adjusted score (once, after both reweights).
        if self.scoring.recency_enabled || self.scoring.reinforcement_enabled {
            facts.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 2) MMR diversity reorder over the (relevance-sorted) pool.
        if self.scoring.mmr_enabled {
            let t0 = Instant::now();
            let n = facts.len();
            let contents: Vec<String> = facts.iter().map(|f| f.fact.content.clone()).collect();
            let relevance: Vec<f32> = facts.iter().map(|f| f.score).collect();
            let order = scoring::mmr_reorder(&contents, &relevance, self.scoring.mmr_lambda);
            let mut slots: Vec<Option<ScoredFact>> = facts.into_iter().map(Some).collect();
            facts = order
                .into_iter()
                .filter_map(|i| slots.get_mut(i).and_then(Option::take))
                .collect();
            sink.record("mmr_diversity", t0.elapsed().as_millis() as u64, n, facts.len());
        }

        facts
    }
```

- [ ] **Step 6: Update any other call sites of `apply_rerank` / `apply_scoring`**

The production call sites are inside `retrieve_inner` (already updated in Step 3). Search the file for remaining callers:

Run: `grep -n "apply_scoring(\|apply_rerank(" src/memory/note_retrieval/mod.rs`

For every call **other than** the two inside `retrieve_inner` and the new tests in Step 1 (which already pass a sink), append the sink argument: add `, &mut TraceSink::Off` as the final argument (these will be in `#[cfg(test)]` code). Each such call must compile with the new signatures.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore note_retrieval::tests::apply_scoring_trace_matches_untraced_and_records_stages note_retrieval::tests::retrieve_traced_on_empty_store_returns_stages`
Expected: PASS (both). Also `cargo check -p alephcore` clean.

- [ ] **Step 8: Commit**

```bash
git add src/memory/note_retrieval/mod.rs
git commit -m "memory: add retrieve_traced with inline per-stage telemetry"
```

---

### Task 3: Real `memory.retrieve_with_trace` handler + registration

**Files:**
- Modify: `src/gateway/handlers/memory_config.rs` (replace placeholder handler; add `UnavailableEmbedder` + `truncate_chars` helper)
- Modify: `src/gateway/handlers/mod.rs` (remove the placeholder registration at lines 584–589)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` (register the real handler in `register_memory_handlers`)

**Interfaces:**
- Consumes: `NoteFactRetrieval::retrieve_traced` + `StageTrace` (Task 2); `MemoryBackend` = `Arc<SqliteMemoryBackend>`; `EmbeddingProvider`; `Config`.
- Produces: `pub async fn handle_retrieve_with_trace(request: JsonRpcRequest, db: MemoryBackend, embedder: Option<Arc<dyn EmbeddingProvider>>, app_config: Arc<tokio::sync::RwLock<crate::Config>>) -> JsonRpcResponse`.

> **Note:** changing the handler signature breaks the old 1-arg registration, so the handler rewrite, the placeholder-registration removal, and the new registration must all land in this one task to keep the build green.

- [ ] **Step 1: Write the failing test (pure truncation helper)**

Add a test module at the bottom of `src/gateway/handlers/memory_config.rs` (or extend an existing `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod retrieve_trace_tests {
    use super::{normalized_query, truncate_chars};

    #[test]
    fn truncate_chars_is_utf8_safe_and_bounds_length() {
        // Multi-byte chars must not panic and must cut on a char boundary.
        let s = "中文字符测试内容"; // 8 chars, 3 bytes each
        let out = truncate_chars(s, 4);
        assert_eq!(out, "中文字符");
        // Shorter-than-limit returns the whole string.
        assert_eq!(truncate_chars("abc", 10), "abc");
        // Exact length returns whole string.
        assert_eq!(truncate_chars("abcd", 4), "abcd");
    }

    #[test]
    fn normalized_query_rejects_blank() {
        assert_eq!(normalized_query(None), None);
        assert_eq!(normalized_query(Some("   ")), None);
        assert_eq!(normalized_query(Some(" hi ")), Some("hi".to_string()));
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p alephcore memory_config::retrieve_trace_tests` — Expected: FAIL to compile (`truncate_chars` / `normalized_query` undefined). RED.

- [ ] **Step 3: Add imports + helpers + the real handler to `memory_config.rs`**

At the top of `memory_config.rs`, ensure these imports exist (the file already imports `JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS` from the protocol and uses `serde_json::json`). Add what's missing:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::AlephError;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::routing::DEFAULT_AGENT_ID;
```

Add the params struct, the truncation helper, the fallback embedder, and the constant — above the handler:

```rust
/// Max chars of note content returned per traced result (debug panel only).
const TRACE_CONTENT_MAX: usize = 280;

#[derive(Debug, Default, Deserialize)]
struct RetrieveTraceParams {
    query: Option<String>,
    agent_id: Option<String>,
    limit: Option<usize>,
}

/// UTF-8-safe truncation to `max` chars (no panic on multi-byte boundaries).
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

/// Trim a raw query param; `None` when absent or blank.
fn normalized_query(raw: Option<&str>) -> Option<String> {
    let q = raw.map_or("", str::trim);
    if q.is_empty() {
        None
    } else {
        Some(q.to_string())
    }
}

/// Stand-in embedder used when no real embedding provider is configured.
/// Its `embed` always errors, which makes `NoteFactRetrieval::retrieve_traced`
/// fall back to FTS-only search (recorded as the `fts_search` stage).
struct UnavailableEmbedder;

#[async_trait]
impl EmbeddingProvider for UnavailableEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Err(AlephError::config("embedding provider not configured"))
    }
    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Err(AlephError::config("embedding provider not configured"))
    }
    fn dimensions(&self) -> usize {
        0
    }
    fn model_name(&self) -> &str {
        "unavailable"
    }
    fn provider_id(&self) -> &str {
        "unavailable"
    }
}
```

Replace the entire placeholder `handle_retrieve_with_trace` (current body at `memory_config.rs:136`) with:

```rust
/// Real retrieval trace: runs the scoring pipeline and returns per-stage
/// telemetry + scored results for the Settings ▸ Memory debug panel.
pub async fn handle_retrieve_with_trace(
    request: JsonRpcRequest,
    db: MemoryBackend,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    app_config: Arc<tokio::sync::RwLock<crate::Config>>,
) -> JsonRpcResponse {
    let params: RetrieveTraceParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let query = match normalized_query(params.query.as_deref()) {
        Some(q) => q,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'query' parameter")
        }
    };
    let agent_id = params
        .agent_id
        .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string());
    let limit = params.limit.unwrap_or(10);

    // Snapshot the three scoring configs, then drop the lock before retrieval.
    let (rerank_cfg, scoring_cfg, expansion_cfg) = {
        let cfg = app_config.read().await;
        (
            cfg.memory.rerank.clone(),
            cfg.memory.retrieval_scoring.clone(),
            cfg.memory.expansion.clone(),
        )
    };

    let memory_dir = match crate::utils::paths::get_note_memory_dir() {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("note memory dir unavailable: {e}"),
            );
        }
    };
    let indexer = Arc::new(NoteIndexer::new(memory_dir, Arc::clone(&db)));
    let embedder: Arc<dyn EmbeddingProvider> =
        embedder.unwrap_or_else(|| Arc::new(UnavailableEmbedder));

    let retrieval = NoteFactRetrieval::new(indexer, embedder)
        .with_rerank_config(&rerank_cfg)
        .with_scoring_config(&scoring_cfg)
        .with_expansion_config(&expansion_cfg);

    let (results, stages) = match retrieval.retrieve_traced(&query, &agent_id, limit).await {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("retrieval failed: {e}"),
            );
        }
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let stages_json: Vec<_> = stages
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "duration_ms": s.duration_ms,
                "input_count": s.input_count,
                "output_count": s.output_count,
            })
        })
        .collect();

    let results_json: Vec<_> = results
        .iter()
        .map(|sf| {
            json!({
                "id": sf.fact.id,
                "content": truncate_chars(&sf.fact.content, TRACE_CONTENT_MAX),
                "score": sf.score,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "query": query,
            "trace": {
                "query": query,
                "timestamp": now_ms,
                "stages": stages_json,
            },
            "results": results_json,
        }),
    )
}
```

- [ ] **Step 4: Remove the placeholder registration in `src/gateway/handlers/mod.rs`**

Delete these lines (currently 584–589):
```rust
        // Memory utility handlers (stateless — no shared state required)
        // NOTE: rerank_config.test is registered in register_config_handlers with vault access
        registry.register(
            "memory.retrieve_with_trace",
            memory_config::handle_retrieve_with_trace,
        );
```
(If the comment lines describe other nearby registrations, keep whatever is unrelated; remove only the `memory.retrieve_with_trace` `registry.register(...)` call. After removal, confirm `memory_config::handle_retrieve_with_trace` has no other reference in this file.)

- [ ] **Step 5: Register the real handler in `register_memory_handlers`**

In `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`, inside `register_memory_handlers` (after the existing `insights.tools` registration block), add a manual closure registration (the `register_handler!` macro can't take the `Option<Arc<..>>` embedder, so register manually):

```rust
    // memory.retrieve_with_trace — real scoring-pipeline trace for the debug panel.
    {
        let memory_db = std::sync::Arc::clone(memory_db);
        let embedder = embedder.clone();
        let app_config = std::sync::Arc::clone(app_config);
        server.handlers_mut().register("memory.retrieve_with_trace", move |req| {
            let memory_db = std::sync::Arc::clone(&memory_db);
            let embedder = embedder.clone();
            let app_config = std::sync::Arc::clone(&app_config);
            async move {
                alephcore::gateway::handlers::memory_config::handle_retrieve_with_trace(
                    req, memory_db, embedder, app_config,
                )
                .await
            }
        });
    }
```

(Confirm `handle_retrieve_with_trace` is reachable as `alephcore::gateway::handlers::memory_config::handle_retrieve_with_trace`; the sibling `insights.tools` uses the analogous `alephcore::gateway::handlers::insights::handle_tools` path. If `memory_config` is not re-exported at that path, use the path the file already uses for memory_config handlers — match the existing `use`/path convention in this file.)

- [ ] **Step 6: Run tests + checks**

Run: `cargo test -p alephcore memory_config::retrieve_trace_tests` — Expected: PASS.
Run: `cargo check -p alephcore` then `cargo check -p alephcore --bin aleph-server` — Expected: both clean.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/memory_config.rs src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs
git commit -m "gateway: wire memory.retrieve_with_trace to real NoteFactRetrieval"
```

---

## Final Verification

- [ ] **Step 1: Contract field check vs panel types.** Read `interfaces/webchat/src/api/memory_config.rs` and confirm the backend JSON (Task 3 Step 3) matches field-by-field:
  - `RetrieveWithTraceResponse`: `query`, `trace`, `results` — all emitted. ✓
  - `RetrievalTrace`: `query`, `timestamp`, `stages` — all emitted. ✓
  - `TraceStage` (required): `name`, `duration_ms`, `input_count`, `output_count` — all emitted; `scores` omitted (has `#[serde(default)]`). ✓
  - `TracedResult`: `id`, `content`, `score` — all emitted. ✓
  No frontend file is modified.

- [ ] **Step 2:** `git log --oneline main..HEAD` — expect Tasks 1–3 as separate commits.

- [ ] **Step 3:** `grep -rn "handle_retrieve_with_trace" src/` — expect: the definition in `memory_config.rs` + the single registration in `builder/handlers/memory.rs`; **no** registration left in `gateway/handlers/mod.rs`.

- [ ] **Step 4:** Final `cargo check -p alephcore --bin aleph-server` clean. Do NOT run full test/clippy suites (resource constraint) unless explicitly asked.

- [ ] **Step 5: Report completion.** Note that live verification (running the daemon + typing a query into Settings ▸ Memory ▸ Retrieval Debug Panel) is the user's call, outside this plan.

## Notes for the implementer

- **Why this is safe without a frontend change:** the panel already calls `memory.retrieve_with_trace` and renders `trace.stages` + scored `results`; it ignores unknown fields and defaults missing ones. Returning the correct shape lights it up.
- **Behavior preservation:** `retrieve()` delegates to `retrieve_inner` with `TraceSink::Off`; the two-pass split in `apply_scoring` is result-identical (per-fact independent adjustments, single re-sort after). The equivalence test (Task 2 Step 1) guards this.
- **Embedder absent:** `UnavailableEmbedder.embed` errors → `retrieve_inner` falls back to `text_retrieve` and records `fts_search`. No error surfaced to the user.
- **Identity:** trace `id` = `ScoredFact.fact.id` (the note path, the pipeline's canonical key).

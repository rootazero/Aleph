# 4-Signal Graph Expansion in Primary Retrieval — Design

**Date:** 2026-06-14
**Status:** Approved (design); pending spec review
**Branch / worktree:** `graph-expansion-retrieval` @ `/Volumes/TBU4/Workspace/Aleph-wt-graphexp`
**Predecessor:** [2026-06-14-note-layer-llm-wiki-protocol-graph-design.md](./2026-06-14-note-layer-llm-wiki-protocol-graph-design.md) — that work built and materialized the 4-signal scorer and wired it into *ingest-time* `gather_related`. This spec is the explicitly-deferred follow-up: inject the same signal into the **primary recall path**.

---

## 1. Goal

A note that does **not** lexically (FTS) or semantically (vector) match the query should still surface in retrieval results when it is strongly tied — by the materialized 4-signal graph relevance — to a note that *did* match. This is associative / multi-hop recall, the canonical knowledge-graph-RAG win.

It must achieve this **without disturbing strong direct hits** and **without changing behavior at all when the graph cache is cold** (pre-first-dream).

## 2. Background: the primary path today

`NoteFactRetrieval::retrieve()` in `src/memory/note_retrieval/mod.rs:249`:

```
embed(query)                          // FTS-only fallback if embedding endpoint down
  → hybrid_search_notes(emb, query, agent, dim, fetch_limit(limit))   // RRF: vector+FTS, content-bearing
  → apply_rerank(query, facts)        // cross-encoder, OPTIONAL (RerankConfig default enabled=false)
  → apply_scoring(ranked, now, counts)// recency / reinforcement / MMR — reweight AFTER rerank
  → truncate(limit)
  → record_recall(...)                // hot-floating producer
```

Facts in / out of `apply_rerank` and `apply_scoring` are `ScoredFact`. The pool entering rerank is `Vec<NoteSearchResult>` (carries full `content`) mapped via `to_scored_fact`.

Established facts that shape this design (verified against current code, not docs):

- The 4-signal scorer (`src/memory/notes/graph/relevance.rs`) is **query-independent** (note↔note). Its output is materialized per dream cycle into `notes_graph_related` and read back via `NoteStore::related_peers(agent_id, node_path, limit) -> Vec<(peer_path, score)>` (`store.rs:316`). Cold cache and the trait default impl both return `vec![]`.
- `apply_scoring` is the existing "reweight after rerank" seam; `recency` + `reinforcement` default **on** with conservative, sub-linear weights (`RetrievalScoringConfig`, `config/types/memory/retrieval.rs`). This is the precedent for how a new signal ships.
- `hybrid_search_notes` returns `NoteSearchResult { path, filename, category, tags, content, score, created_at, updated_at }` (`notes/search_result.rs`). There is **no** content-by-path fetch on the `NoteStore` trait — `get_note_index` returns metadata only.
- The reranker is **off by default** (`RerankConfig::default().enabled == false`), so expansion candidates cannot rely on a cross-encoder to score them; they must carry a self-sufficient propagated score.

## 3. Decisions (locked with user)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Mechanism | **Associative expansion** of the candidate pool before rerank (not coherence-reweight, not both). |
| D2 | Default posture | **Default-on, conservative blended** — expansion competes on a propagated score scaled below its seed; can displace a *weak* direct hit but never a strong seed. |
| D3 | Signal scope | **4-signal `related_peers` only.** `community_peers` (Louvain membership) is *not* added to the primary path (YAGNI). |
| D4 | Cargo | Tests ship with code; **not run locally** per standing machine-load preference. Static-audit verification only. User runs the suite when ready. |
| D5 | Isolation | Worktree `graph-expansion-retrieval`; never touch `main` directly until user says merge. |

## 4. Architecture

A new **expansion stage** slots between `hybrid_search_notes` and `apply_rerank`. Everything downstream (rerank, scoring, truncate, record_recall) is unchanged and operates on the merged pool.

```
hits  = hybrid_search_notes(...)                        // direct, content-bearing
peers = graph_expand(store, agent, &hits, &cfg)         // NEW — 4-signal associative recall
pool  = (hits ++ peers), capped to RERANK_MAX_CANDIDATES by score
facts = pool.map(to_scored_fact)
  → apply_rerank → apply_scoring → truncate(limit) → record_recall   // all unchanged
```

### 4.1 New module `src/memory/note_retrieval/expansion.rs`

```rust
use crate::config::types::memory::ExpansionConfig;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteSearchResult;
use std::collections::HashSet;

/// Associative recall: for the top hits, pull their strongest 4-signal related
/// peers into the candidate pool with a propagated score, hydrated with content.
///
/// Query-independent: graph relatedness measures note↔note, so a peer surfaces
/// purely because it is tied to a *query-relevant* seed. Conservative by
/// construction — a peer's score is scaled strictly below its seed.
///
/// Never fails retrieval: store errors are swallowed (logged) and treated as
/// "no expansion", matching `retrieve()`'s embedding-fallback philosophy. A cold
/// cache (`related_peers` empty) yields zero expansion -> legacy behavior.
pub async fn graph_expand<S: NoteStore + Send + Sync + ?Sized>(
    store: &S,
    agent_id: &str,
    hits: &[NoteSearchResult],
    cfg: &ExpansionConfig,
) -> Vec<NoteSearchResult> {
    if !cfg.enabled || hits.is_empty() || cfg.max_expanded == 0 {
        return Vec::new();
    }

    // Dedup target: never re-surface a path already among the direct hits.
    let mut seen: HashSet<String> = hits.iter().map(|h| h.path.clone()).collect();

    // (peer_path, propagated_score), in discovery order. Seeds iterate in hit
    // (RRF-desc) order, so a peer tied to multiple seeds is captured via its
    // strongest seed first (the `seen` insert below blocks weaker re-captures).
    let mut collected: Vec<(String, f32)> = Vec::new();

    'outer: for seed in hits.iter().take(cfg.max_seeds) {
        let peers = match store.related_peers(agent_id, &seed.path, cfg.peers_per_seed).await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, seed = %seed.path,
                    "graph expansion: related_peers failed (non-fatal)");
                continue;
            }
        };
        // Normalize by the seed's strongest edge so unbounded 4-signal
        // magnitudes can't dominate; the top peer of a seed maxes at
        // `weight * seed.score`.
        let seed_top_edge = peers.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
        if seed_top_edge <= 0.0 {
            continue;
        }
        for (peer, edge) in peers {
            if collected.len() >= cfg.max_expanded {
                break 'outer;
            }
            if seen.insert(peer.clone()) {
                let propagated = seed.score * cfg.weight * (edge / seed_top_edge);
                collected.push((peer, propagated));
            }
        }
    }

    if collected.is_empty() {
        return Vec::new();
    }

    // Batch-hydrate content (expansion peers need full content for the agent and
    // for the optional reranker). Missing/deleted paths are silently dropped.
    let paths: Vec<String> = collected.iter().map(|(p, _)| p.clone()).collect();
    let hydrated = match store.get_notes_with_content(agent_id, &paths).await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(error = %e, "graph expansion: content hydration failed (non-fatal)");
            return Vec::new();
        }
    };

    // Stamp the propagated score onto each hydrated result, preserving discovery
    // order. `get_notes_with_content` returns score 0.0; we overwrite it.
    let score_by_path: std::collections::HashMap<&str, f32> =
        collected.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let mut out: Vec<NoteSearchResult> = hydrated
        .into_iter()
        .filter_map(|mut r| {
            let s = *score_by_path.get(r.path.as_str())?;
            r.score = s;
            Some(r)
        })
        .collect();
    // Deterministic: discovery order is already score-desc-ish but hydration may
    // reorder; sort by propagated score desc, path asc to break ties.
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}
```

**Conservative-blended property (D2):** a peer's propagated score is `seed.score × weight × (edge/seed_top_edge) ∈ [0, weight × seed.score]`. With `weight = 0.5`, an expanded peer maxes at half its seed's score, so it can outrank a *weak* direct hit but never a strong seed. This is the mathematical realization of "conservative."

### 4.2 New store method — content by path

`NoteStore` trait (`src/memory/notes/store.rs`), grouped with the Phase-4 graph methods:

```rust
/// Batch-fetch notes (with full content) by exact path, for `agent_id`.
/// Unknown/deleted paths are omitted. The `score` field is 0.0 (callers that
/// need a score assign their own). Mirrors the row -> NoteSearchResult shape of
/// `hybrid_search_notes`. Default impl returns empty so non-SQLite mocks compile.
async fn get_notes_with_content(
    &self,
    agent_id: &str,
    paths: &[String],
) -> Result<Vec<NoteSearchResult>, AlephError> {
    let _ = (agent_id, paths);
    Ok(Vec::new())
}
```

Real impl on `SqliteMemoryBackend` (`src/memory/store/sqlite/notes.rs`): a single parameterized `SELECT` over `notes_index` filtered by `agent_id` + `path IN (...)`, projecting the same columns `hybrid_search_notes` already maps to `NoteSearchResult`. The exact column list and row-mapping closure are copied from the existing `hybrid_search_notes` body (the plan extracts the precise SQL after reading `notes.rs:768`). Empty `paths` short-circuits to `Ok(vec![])` (avoids an `IN ()` syntax error).

### 4.3 Config — `ExpansionConfig`

New struct in `src/config/types/memory/retrieval.rs` (sibling to `RetrievalScoringConfig`), re-exported from `config/types/memory/mod.rs` alongside `RetrievalScoringConfig`:

```rust
const fn default_expansion_enabled() -> bool { true }
const fn default_max_seeds() -> usize { 5 }
const fn default_peers_per_seed() -> usize { 3 }
const fn default_max_expanded() -> usize { 8 }
const fn default_expansion_weight() -> f32 { 0.5 }

/// Associative graph expansion of the retrieval candidate pool. Pulls the
/// strongest 4-signal related peers of the top direct hits into the pool before
/// rerank, so notes tied to a match surface even without lexical/semantic
/// overlap. Default-on and conservative: a peer's score is scaled strictly below
/// its seed, and a cold graph cache makes this a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExpansionConfig {
    /// Master switch. Default `true`. `false` restores the legacy path
    /// (zero expansion, byte-for-byte).
    #[serde(default = "default_expansion_enabled")]
    pub enabled: bool,
    /// How many top hits seed expansion. Default 5.
    #[serde(default = "default_max_seeds")]
    pub max_seeds: usize,
    /// Related peers pulled per seed (passed to `related_peers`). Default 3.
    #[serde(default = "default_peers_per_seed")]
    pub peers_per_seed: usize,
    /// Hard cap on total expansion candidates added to the pool. Default 8.
    #[serde(default = "default_max_expanded")]
    pub max_expanded: usize,
    /// Propagation strength in `[0,1]`: a peer's score is
    /// `seed.score * weight * (edge / seed_top_edge)`. Default 0.5 (a peer maxes
    /// at half its seed's score). Clamped to `[0,1]` at use.
    #[serde(default = "default_expansion_weight")]
    pub weight: f32,
}

impl ExpansionConfig {
    /// True when expansion will do any work.
    pub const fn is_active(&self) -> bool {
        self.enabled && self.max_seeds > 0 && self.max_expanded > 0
    }
}

impl Default for ExpansionConfig {
    fn default() -> Self {
        Self {
            enabled: default_expansion_enabled(),
            max_seeds: default_max_seeds(),
            peers_per_seed: default_peers_per_seed(),
            max_expanded: default_max_expanded(),
            weight: default_expansion_weight(),
        }
    }
}
```

`weight` is clamped to `[0,1]` where consumed (in the builder), mirroring how `rerank_weight` is clamped in `with_reranker`.

### 4.4 Wiring into `NoteFactRetrieval`

`src/memory/note_retrieval/mod.rs`:

- `pub mod expansion;`
- New field `expansion: ExpansionConfig`; `new()` sets `ExpansionConfig::default()` (on), so cold-start and existing call sites get expansion without extra plumbing — exactly how `scoring` defaults on.
- New builder:

```rust
/// Attach associative graph-expansion config. Default `new()` is already on;
/// this lets callers tune or disable it. `weight` is clamped to `[0,1]`.
#[must_use]
pub fn with_expansion_config(mut self, cfg: &ExpansionConfig) -> Self {
    self.expansion = cfg.clone();
    self.expansion.weight = self.expansion.weight.clamp(0.0, 1.0);
    self
}
```

- `retrieve()` — insert the stage after `hybrid_search_notes`, before mapping to facts:

```rust
let mut results = self
    .indexer
    .store()
    .hybrid_search_notes(&embedding, query, agent_id, dim, self.fetch_limit(limit))
    .await?;

if self.expansion.is_active() {
    let peers = expansion::graph_expand(
        self.indexer.store().as_ref(),
        agent_id,
        &results,
        &self.expansion,
    ).await;
    results.extend(peers);
    // Bound the merged pool so rerank cost stays capped regardless of expansion.
    if results.len() > RERANK_MAX_CANDIDATES {
        results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(RERANK_MAX_CANDIDATES);
    }
}

let facts: Vec<ScoredFact> = results.iter().map(|r| r.to_scored_fact(agent_id)).collect();
let ranked = self.apply_rerank(query, facts).await;
// ... unchanged
```

(`store()` returns `Arc<S>`; `.as_ref()` yields `&S` which satisfies `?Sized` generic. Confirm `store()` accessor exists — it is used elsewhere in this file, e.g. `self.indexer.store().recall_hit_counts`.)

- `retrieve_multi_agent()` — expand **per agent** inside the existing loop (peers are per-agent), before pushing into `all_results`:

```rust
for agent_id in agent_ids {
    let mut results = self
        .indexer
        .store()
        .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
        .await?;
    if self.expansion.is_active() {
        let peers = expansion::graph_expand(
            self.indexer.store().as_ref(), agent_id, &results, &self.expansion).await;
        results.extend(peers);
    }
    for r in results {
        all_results.push(r.to_scored_fact(agent_id));
    }
}
```

The existing `all_results.truncate(self.fetch_limit(limit))` after the loop already bounds the merged multi-agent pool before rerank.

### 4.5 Config plumbing at construction sites

Production constructors that already wire rerank/scoring also wire expansion. From the grep, the production sites are:

- `src/builtin_tools/memory_search.rs:223` — already calls `.with_rerank_config(cfg)`. Add `.with_expansion_config(&expansion_cfg)` sourced from the same memory config block that yields the rerank config. If the memory config has no expansion section yet, source `ExpansionConfig::default()` (on) — wiring is for *tunability*; default-on already holds without it.
- `src/memory/session_search_summary/filter.rs:118` — constructs `NoteFactRetrieval::new(...)`; since `new()` defaults expansion on, this path gets expansion automatically. No change required unless the surrounding config exposes an expansion override (plan checks and wires if present).

The plan resolves where `RerankConfig` is read from the global config and adds an `ExpansionConfig` field there (default-on) so it round-trips through TOML/JSON schema, mirroring `RetrievalScoringConfig`'s placement.

## 5. Data flow (final)

```
retrieve(query, agent, limit):
  emb   = embed(query)                       // FTS-only fallback on embedding failure (unchanged)
  hits  = hybrid_search_notes(emb, query, agent, fetch_limit(limit))     // content-bearing
  if expansion active:
     peers = graph_expand(store, agent, hits, cfg)   // 4-signal related_peers + content hydrate
     pool  = (hits ++ peers) truncated to RERANK_MAX_CANDIDATES by score
  else:
     pool  = hits                             // legacy, byte-for-byte
  facts = pool.map(to_scored_fact)
  ranked = apply_rerank(query, facts)         // unchanged (cross-encoder re-judges if configured)
  ranked = apply_scoring(ranked, now, counts) // unchanged (recency/reinforcement/MMR)
  ranked.truncate(limit)
  record_recall(query, agent, ranked)         // surfaced graph peers heat up too — desirable
  Ok(ranked)
```

## 6. Error handling & degradation

- `related_peers` / `get_notes_with_content` errors inside `graph_expand` are logged at `debug` and treated as empty — **core recall never fails because of a graph-cache problem** (distinct from `gather_related`, which `?`-propagates; the primary path is more conservative).
- Cold cache (pre-first-dream) → `related_peers` empty → `graph_expand` returns `Vec::new()` → legacy ordering.
- `enabled = false` (or `max_seeds`/`max_expanded` 0) → `is_active()` false → stage skipped entirely → legacy byte-for-byte.
- Hydration misses (peer deleted between dream and query) → that path is dropped via `filter_map`.

## 7. Testing (TDD; ships with code, not run locally per D4)

**`expansion.rs` unit tests** (mock `NoteStore` with programmable `related_peers` + `get_notes_with_content`):

1. `empty_hits_yields_no_expansion`
2. `disabled_config_yields_no_expansion`
3. `cold_cache_related_peers_empty_yields_no_expansion` (legacy preservation)
4. `propagation_scales_peer_below_seed` — peer score `== seed.score * weight * (edge/seed_top_edge)`; assert `peer.score < seed.score` for `weight ≤ 1`.
5. `peer_already_in_hits_is_not_re_added` (dedup vs direct hits)
6. `peer_shared_by_two_seeds_captured_via_strongest_seed_once` (dedup + first-wins)
7. `global_max_expanded_cap_respected`
8. `hydration_miss_is_dropped` (peer path absent from `get_notes_with_content` result)
9. `output_sorted_by_score_desc_then_path`

**`retrieve()` integration test** (real `SqliteMemoryBackend`, `MockEmbeddingProvider`):

10. Index notes A, B; materialize a `notes_graph_related` edge A→B (via `replace_graph_related`); B's content does **not** match the query but A does. Assert B appears in `retrieve()` results **with** the materialized edge, and is **absent** when the graph cache is empty (legacy parity).

**Store method test** (`sqlite/notes.rs` tests):

11. `get_notes_with_content_returns_content_for_known_paths_skips_unknown` — known paths hydrate with content; an unknown path is omitted; empty input returns empty.

## 8. Entropy / dead-code accounting

Purely additive. No code is obsoleted:

- `gather_related` (`notes/ingest/retrieve.rs`) keeps its own `related_peers` use — that is the **ingest-time** context-builder, a different consumer from retrieval. Both are legitimate.
- No existing retrieval branch is removed; the legacy path is preserved verbatim behind `is_active()`.

No dead code is introduced (every new symbol — `graph_expand`, `get_notes_with_content`, `ExpansionConfig`, `with_expansion_config` — has a wired consumer in this same change). This is itself the lesson from the predecessor spec: do not build an orphan.

## 9. File manifest

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `src/memory/note_retrieval/expansion.rs` | `graph_expand` + unit tests |
| Modify | `src/config/types/memory/retrieval.rs` | `ExpansionConfig` struct + defaults + tests |
| Modify | `src/config/types/memory/mod.rs` | re-export `ExpansionConfig` |
| Modify | `src/memory/notes/store.rs` | `get_notes_with_content` trait method (default empty) |
| Modify | `src/memory/store/sqlite/notes.rs` | real `get_notes_with_content` impl + test |
| Modify | `src/memory/note_retrieval/mod.rs` | `pub mod expansion;`, field, builder, wire `retrieve()` + `retrieve_multi_agent()`, integration test |
| Modify | `src/builtin_tools/memory_search.rs` | `.with_expansion_config(...)` at the production constructor |
| Modify | `src/config/types/memory/*` (rerank source site) | add `ExpansionConfig` to the config that already carries `RerankConfig`/`RetrievalScoringConfig` |
| Modify | `docs/reference/memory/RETRIEVAL.md` | document the expansion stage |

## 10. Out of scope (explicit)

- `community_peers` (Louvain membership) in the primary path — 4-signal only (D3).
- Coherence-reweight of existing candidates — not chosen (D1).
- Changing `gather_related` (ingest path) — untouched.
- Re-running the dream graph recompute — expansion only *reads* the materialized `notes_graph_related`; production of that table is the predecessor's `GraphRecomputeStage`, unchanged.

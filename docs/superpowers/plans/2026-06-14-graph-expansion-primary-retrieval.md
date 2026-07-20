# 4-Signal Graph Expansion in Primary Retrieval — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inject the materialized 4-signal graph relevance into `NoteFactRetrieval::retrieve()` as an associative-expansion stage, so notes strongly tied to a direct hit surface even without lexical/semantic query overlap.

**Architecture:** A new `graph_expand` stage between `hybrid_search_notes` and `apply_rerank` pulls each top hit's strongest `related_peers` into the candidate pool with a propagated score (scaled strictly below its seed), hydrated with content via a new `get_notes_with_content` store method. Default-on and conservative; a cold graph cache or `enabled=false` yields legacy behavior byte-for-byte. Config threads through the same two production chains that already carry `retrieval_scoring`/`rerank`.

**Tech Stack:** Rust, Tokio, `async_trait`, rusqlite, serde/schemars. Spec: `docs/superpowers/specs/2026-06-14-graph-expansion-primary-retrieval-design.md`.

---

## GLOBAL EXECUTION CONSTRAINTS (every task)

- **Worktree:** `/Volumes/TBU4/Workspace/Aleph-wt-graphexp`, branch `graph-expansion-retrieval`. All paths below are relative to it. Bash resets cwd to the main repo after each command — **always `cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp` first** and use absolute paths.
- **NEVER touch `main`.** Commit only on `graph-expansion-retrieval`.
- **NO cargo.** Do **not** run `cargo check/test/clippy/build`. Verification is **static audit** (grep match-arms, field counts, type/signature parity). Tests are written (TDD red→green discipline preserved in code) and **ship with the change**; the user runs the suite later.
- **Commits:** English, format `<scope>: <desc>`, **no** `Co-Authored-By` trailer. One commit per task. `git commit --no-verify` to skip hooks.
- Each task is self-contained and compiles on its own (no forward references to later tasks).

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/memory/notes/store.rs` | `get_notes_with_content` trait method (default empty) |
| Modify | `src/memory/store/sqlite/notes.rs` | real `get_notes_with_content` + its test |
| Modify | `src/config/types/memory/retrieval.rs` | `ExpansionConfig` struct + defaults + tests |
| Modify | `src/config/types/memory/mod.rs` | re-export `ExpansionConfig`; `MemoryConfig.expansion` field + Default + `assembler_config()` sync |
| Create | `src/memory/note_retrieval/expansion.rs` | `graph_expand` + unit tests |
| Modify | `src/memory/note_retrieval/mod.rs` | `pub mod expansion;`, field, builder, wire `retrieve()`/`retrieve_multi_agent()`, integration test |
| Modify | `src/builtin_tools/memory_search.rs` | `new_with_config` expansion param + `.with_expansion_config` |
| Modify | `src/executor/builtin_registry/builder/constructor.rs` | read `memory.expansion`, pass to `new_with_config` |
| Modify | `src/config/types/memory/assembler.rs` | `AssemblerConfig.expansion` mirror field + Default |
| Modify | `src/thinker/memory_context_provider/constructor.rs` | `.with_expansion_config(&assembler_config.expansion)` |
| Modify | `src/memory/session_search_summary/filter.rs` | add `expansion: Default::default()` to `AssemblerConfig` literal |
| Modify | `src/memory/assembler/tests.rs` | add `expansion: Default::default()` to `AssemblerConfig` literal |
| Modify | `docs/reference/memory/RETRIEVAL.md` | document the expansion stage |

---

## Task 1: `get_notes_with_content` store method

Batch-fetch notes (with content) by exact path. Mirrors the `hybrid_search_notes` row-building loop (`get_note_index` + `load_note_content_from_disk`). Required because expansion peers must carry content, and `get_note_index` returns metadata only.

**Files:**
- Modify: `src/memory/notes/store.rs` (trait, after `related_peers`, ~line 324)
- Modify: `src/memory/store/sqlite/notes.rs` (impl, after `vector_search_notes_with_content`, ~line 857; test in the `#[cfg(test)]` module)

- [ ] **Step 1: Add the trait method (default impl) to `store.rs`**

Insert immediately after the `related_peers` method closes (the `Ok(vec![])` block ending ~line 324), still inside the `NoteStore` trait:

```rust
    /// Batch-fetch notes (with full content) by exact path, for `agent_id`.
    /// Unknown/deleted paths are omitted; order follows `paths`. The `score`
    /// field is `0.0` (callers assign their own). Mirrors the row ->
    /// `NoteSearchResult` shape of `hybrid_search_notes`. Default impl returns
    /// empty so non-`SQLite` stores / test mocks keep compiling.
    async fn get_notes_with_content(
        &self,
        agent_id: &str,
        paths: &[String],
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let _ = (agent_id, paths);
        Ok(Vec::new())
    }
```

- [ ] **Step 2: Add the real impl to `sqlite/notes.rs`**

Insert immediately after `vector_search_notes_with_content` closes (~line 857), inside the `#[async_trait] impl NoteStore for SqliteMemoryBackend` block:

```rust
    async fn get_notes_with_content(
        &self,
        agent_id: &str,
        paths: &[String],
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let mut results = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(entry) = self.get_note_index(path, agent_id).await? {
                let content = load_note_content_from_disk(&entry, agent_id)
                    .await
                    .unwrap_or_default();
                results.push(crate::memory::notes::NoteSearchResult {
                    path: entry.path.clone(),
                    filename: entry.filename.clone(),
                    category: entry.category.clone(),
                    tags: entry.tags.clone(),
                    content,
                    score: 0.0,
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                });
            }
        }
        Ok(results)
    }
```

- [ ] **Step 3: Write the test (TDD)**

Find the `#[cfg(test)] mod tests` block in `src/memory/store/sqlite/notes.rs` (search `mod tests`). Add this test. It asserts **membership semantics** (known paths returned, unknown skipped, empty→empty) — not content bytes, since `load_note_content_from_disk` reads the global note dir which tests don't control. Use the same `index_note` + tempdir pattern other tests in this module use (search an existing `async fn` test for the exact backend-construction lines and mirror them).

```rust
    #[tokio::test]
    async fn get_notes_with_content_returns_known_paths_skips_unknown() {
        use crate::memory::notes::KnowledgeNote;
        let dir = tempfile::tempdir().unwrap();
        let backend = SqliteMemoryBackend::new(dir.path()).unwrap();

        for title in ["alpha", "beta"] {
            let note = KnowledgeNote {
                title: title.to_string(),
                category: "general".to_string(),
                facts: vec![format!("{title} fact")],
                content_hash: format!("hash_{title}"),
                ..Default::default()
            };
            backend.index_note(&note, "default", "general").await.unwrap();
        }

        // index_note stores path = "{category}/{title}" (extensionless,
        // verified in sqlite/notes.rs index_note).
        let query = vec![
            "general/alpha".to_string(),
            "general/beta".to_string(),
            "general/does-not-exist".to_string(),
        ];
        let got = backend
            .get_notes_with_content("default", &query)
            .await
            .unwrap();
        let got_paths: std::collections::HashSet<&str> =
            got.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(got.len(), 2, "unknown path must be skipped");
        assert!(got_paths.contains("general/alpha"));
        assert!(got_paths.contains("general/beta"));

        // Empty input -> empty output (no IN () footgun).
        let empty = backend.get_notes_with_content("default", &[]).await.unwrap();
        assert!(empty.is_empty());
    }
```

- [ ] **Step 4: Static audit (no cargo)**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
grep -n "async fn get_notes_with_content" src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
```
Expected: exactly **2** hits (one trait default, one impl). Confirm both signatures are character-for-character identical (`agent_id: &str, paths: &[String]) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError>`).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
git commit --no-verify -m "feat(memory): add NoteStore::get_notes_with_content batch fetch"
```

---

## Task 2: `ExpansionConfig`

The tuning knobs for graph expansion. Default-on, conservative.

**Files:**
- Modify: `src/config/types/memory/retrieval.rs` (append struct + defaults + tests)
- Modify: `src/config/types/memory/mod.rs:24` (re-export)

- [ ] **Step 1: Write the tests first (TDD), appended to the `#[cfg(test)] mod tests` in `retrieval.rs`**

```rust
    #[test]
    fn expansion_default_is_on_and_active() {
        let c = ExpansionConfig::default();
        assert!(c.enabled);
        assert!(c.is_active());
        assert_eq!(c.max_seeds, 5);
        assert_eq!(c.peers_per_seed, 3);
        assert_eq!(c.max_expanded, 8);
        assert!((c.weight - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn expansion_inactive_when_disabled_or_zero_caps() {
        assert!(!ExpansionConfig { enabled: false, ..Default::default() }.is_active());
        assert!(!ExpansionConfig { max_seeds: 0, ..Default::default() }.is_active());
        assert!(!ExpansionConfig { max_expanded: 0, ..Default::default() }.is_active());
    }
```

- [ ] **Step 2: Append the struct + defaults to `retrieval.rs`** (after `RetrievalScoringConfig`'s `impl Default`, before the `#[cfg(test)]` module)

```rust
const fn default_expansion_enabled() -> bool {
    true
}
const fn default_max_seeds() -> usize {
    5
}
const fn default_peers_per_seed() -> usize {
    3
}
const fn default_max_expanded() -> usize {
    8
}
const fn default_expansion_weight() -> f32 {
    0.5
}

/// Associative graph expansion of the retrieval candidate pool. Pulls the
/// strongest 4-signal related peers of the top direct hits into the pool before
/// rerank, so notes tied to a match surface even without lexical/semantic
/// overlap. Default-on and conservative: a peer's propagated score is scaled
/// strictly below its seed, and a cold graph cache makes the stage a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExpansionConfig {
    /// Master switch. Default `true`. `false` restores the legacy path (zero
    /// expansion, byte-for-byte).
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
    /// at half its seed's score). Clamped to `[0,1]` by `with_expansion_config`.
    #[serde(default = "default_expansion_weight")]
    pub weight: f32,
}

impl ExpansionConfig {
    /// True when expansion will do any work.
    #[must_use]
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

- [ ] **Step 3: Re-export from `mod.rs`** — change line 24 from:

```rust
pub use retrieval::RetrievalScoringConfig;
```
to:
```rust
pub use retrieval::{ExpansionConfig, RetrievalScoringConfig};
```

- [ ] **Step 4: Static audit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
grep -n "pub struct ExpansionConfig\|pub use retrieval::{ExpansionConfig" src/config/types/memory/retrieval.rs src/config/types/memory/mod.rs
```
Expected: struct defined once; re-export present.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/config/types/memory/retrieval.rs src/config/types/memory/mod.rs
git commit --no-verify -m "feat(config): add ExpansionConfig for graph retrieval expansion"
```

---

## Task 3: `graph_expand` module

The pure expansion logic + its unit tests. Depends on Task 1 (`get_notes_with_content`) and Task 2 (`ExpansionConfig`).

**Files:**
- Create: `src/memory/note_retrieval/expansion.rs`
- (the `pub mod expansion;` declaration is added in Task 4)

- [ ] **Step 1: Create `expansion.rs` with the function**

```rust
//! Associative graph expansion of the retrieval candidate pool.
//!
//! Query-independent: 4-signal relatedness measures note<->note, so a peer
//! surfaces purely because it is tied to a *query-relevant* seed. Conservative
//! by construction — a peer's score is scaled strictly below its seed. Never
//! fails retrieval: store errors are swallowed (logged) and treated as "no
//! expansion", matching `retrieve()`'s embedding-fallback philosophy. A cold
//! cache (`related_peers` empty) yields zero expansion -> legacy behavior.

use std::collections::{HashMap, HashSet};

use crate::config::types::memory::ExpansionConfig;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::NoteSearchResult;

/// Expand `hits` with the strongest 4-signal related peers of the top seeds.
/// Returns hydrated `NoteSearchResult`s (content carried) stamped with a
/// propagated score, sorted by score desc then path asc. Empty when expansion
/// is inactive, hits are empty, the cache is cold, or every peer fails to
/// hydrate.
pub async fn graph_expand<S: NoteStore + Send + Sync>(
    store: &S,
    agent_id: &str,
    hits: &[NoteSearchResult],
    cfg: &ExpansionConfig,
) -> Vec<NoteSearchResult> {
    if !cfg.is_active() || hits.is_empty() {
        return Vec::new();
    }

    // Dedup target: never re-surface a path already among the direct hits.
    let mut seen: HashSet<String> = hits.iter().map(|h| h.path.clone()).collect();
    // (peer_path, propagated_score) in discovery order. Seeds iterate in hit
    // (RRF-desc) order, so a peer tied to multiple seeds is captured via its
    // strongest seed first; the `seen` insert blocks weaker re-captures.
    let mut collected: Vec<(String, f32)> = Vec::new();

    'outer: for seed in hits.iter().take(cfg.max_seeds) {
        let peers = match store
            .related_peers(agent_id, &seed.path, cfg.peers_per_seed)
            .await
        {
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

    // Hydrate content for the collected peers (they need full content for the
    // agent and the optional reranker). Missing/deleted paths are dropped.
    let paths: Vec<String> = collected.iter().map(|(p, _)| p.clone()).collect();
    let hydrated = match store.get_notes_with_content(agent_id, &paths).await {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!(error = %e,
                "graph expansion: content hydration failed (non-fatal)");
            return Vec::new();
        }
    };

    let score_by_path: HashMap<&str, f32> =
        collected.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let mut out: Vec<NoteSearchResult> = hydrated
        .into_iter()
        .filter_map(|mut r| {
            let s = *score_by_path.get(r.path.as_str())?;
            r.score = s;
            Some(r)
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}
```

- [ ] **Step 2: Append the unit tests** to `expansion.rs`

These use a real `SqliteMemoryBackend` (the only `NoteStore` impl; no reusable mock exists). Hits are constructed directly so seed scores are controlled; peers are seeded via `replace_graph_related` and indexed via `index_note`. Content-on-disk is not asserted (global-dir dependent); assertions target **path membership + propagated score**, which need only the index.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;
    use tempfile::TempDir;

    fn hit(path: &str, score: f32) -> NoteSearchResult {
        NoteSearchResult {
            path: path.to_string(),
            filename: path.rsplit('/').next().unwrap_or(path).to_string(),
            category: path.split('/').next().unwrap_or("general").to_string(),
            tags: vec![],
            content: String::new(),
            score,
            created_at: 0,
            updated_at: 0,
        }
    }

    async fn backend() -> (SqliteMemoryBackend, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let b = SqliteMemoryBackend::new(dir.path()).unwrap();
        (b, dir)
    }

    /// Index a note so `get_note_index`/`get_notes_with_content` resolve `path`.
    /// Returns the path the index assigned (category/filename).
    async fn index(b: &SqliteMemoryBackend, title: &str) -> String {
        let note = KnowledgeNote {
            title: title.to_string(),
            category: "general".to_string(),
            facts: vec![format!("{title} fact")],
            content_hash: format!("h_{title}"),
            ..Default::default()
        };
        b.index_note(&note, "default", "general").await.unwrap();
        format!("general/{title}")
    }

    #[tokio::test]
    async fn empty_hits_yields_no_expansion() {
        let (b, _d) = backend().await;
        let out = graph_expand(&b, "default", &[], &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn disabled_config_yields_no_expansion() {
        let (b, _d) = backend().await;
        let cfg = ExpansionConfig { enabled: false, ..Default::default() };
        let out = graph_expand(&b, "default", &[hit("general/a", 0.9)], &cfg).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn cold_cache_yields_no_expansion() {
        // related_peers returns empty when nothing materialized -> legacy.
        let (b, _d) = backend().await;
        let _ = index(&b, "a").await;
        let out = graph_expand(&b, "default", &[hit("general/a", 0.9)],
            &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn propagation_scales_peer_below_seed() {
        let (b, _d) = backend().await;
        let a = index(&b, "a").await;
        let bp = index(&b, "b").await;
        // Single peer: seed_top_edge == edge -> factor 1.0 -> 0.8 * 0.5 = 0.4.
        b.replace_graph_related("default", &[(a.clone(), bp.clone(), 4.0)])
            .await
            .unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.8)],
            &ExpansionConfig::default()).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, bp);
        assert!((out[0].score - 0.4).abs() < 1e-6, "got {}", out[0].score);
        assert!(out[0].score < 0.8, "peer must not outrank its seed");
    }

    #[tokio::test]
    async fn two_peers_normalized_by_top_edge() {
        let (b, _d) = backend().await;
        let a = index(&b, "a").await;
        let p1 = index(&b, "p1").await;
        let p2 = index(&b, "p2").await;
        b.replace_graph_related("default", &[
            (a.clone(), p1.clone(), 4.0),
            (a.clone(), p2.clone(), 2.0),
        ]).await.unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.8)],
            &ExpansionConfig::default()).await;
        let by: std::collections::HashMap<&str, f32> =
            out.iter().map(|r| (r.path.as_str(), r.score)).collect();
        // p1: 0.8*0.5*(4/4)=0.4 ; p2: 0.8*0.5*(2/4)=0.2
        assert!((by[p1.as_str()] - 0.4).abs() < 1e-6);
        assert!((by[p2.as_str()] - 0.2).abs() < 1e-6);
        // Sorted desc -> p1 first.
        assert_eq!(out[0].path, p1);
    }

    #[tokio::test]
    async fn peer_already_in_hits_is_not_re_added() {
        let (b, _d) = backend().await;
        let a = index(&b, "a").await;
        let bp = index(&b, "b").await;
        b.replace_graph_related("default", &[(a.clone(), bp.clone(), 4.0)])
            .await
            .unwrap();
        // b is already a direct hit -> expansion must not duplicate it.
        let hits = vec![hit(&a, 0.9), hit(&bp, 0.7)];
        let out = graph_expand(&b, "default", &hits, &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn global_max_expanded_cap_respected() {
        let (b, _d) = backend().await;
        let a = index(&b, "a").await;
        let mut rows = Vec::new();
        for i in 0..6 {
            let p = index(&b, &format!("p{i}")).await;
            rows.push((a.clone(), p, (10 - i) as f32));
        }
        b.replace_graph_related("default", &rows).await.unwrap();
        let cfg = ExpansionConfig { peers_per_seed: 10, max_expanded: 3, ..Default::default() };
        let out = graph_expand(&b, "default", &[hit(&a, 0.9)], &cfg).await;
        assert_eq!(out.len(), 3, "global cap must bound total expansion");
    }

    #[tokio::test]
    async fn hydration_miss_is_dropped() {
        let (b, _d) = backend().await;
        let a = index(&b, "a").await;
        // Peer path is materialized but never indexed -> get_notes_with_content
        // skips it -> dropped from output.
        b.replace_graph_related("default", &[(a.clone(), "general/ghost".to_string(), 4.0)])
            .await
            .unwrap();
        let out = graph_expand(&b, "default", &[hit(&a, 0.9)],
            &ExpansionConfig::default()).await;
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 3: Static audit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
grep -n "pub async fn graph_expand\|fn is_active\|related_peers\|get_notes_with_content" src/memory/note_retrieval/expansion.rs
```
Expected: `graph_expand` defined; calls `cfg.is_active()`, `store.related_peers`, `store.get_notes_with_content`. Confirm the propagation line reads `seed.score * cfg.weight * (edge / seed_top_edge)` and the `seed_top_edge <= 0.0` guard is present.

- [ ] **Step 4: Commit** (module compiles only once declared in Task 4; commit together with Task 4? No — declare the module here to keep this task self-contained.)

Add the module declaration now so this file is reachable. Edit `src/memory/note_retrieval/mod.rs` — after the existing `pub mod scoring;` (line 8) add:

```rust
pub mod expansion;
```

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/memory/note_retrieval/expansion.rs src/memory/note_retrieval/mod.rs
git commit --no-verify -m "feat(memory): add graph_expand 4-signal associative recall"
```

---

## Task 4: Wire expansion into `NoteFactRetrieval`

Add the config field + builder, run `graph_expand` in `retrieve()` and `retrieve_multi_agent()`, and an integration test. Depends on Tasks 1–3.

**Files:**
- Modify: `src/memory/note_retrieval/mod.rs`

- [ ] **Step 1: Import `ExpansionConfig` + add the field**

In the `use` block (near line 12, `use crate::config::types::memory::RetrievalScoringConfig;`) change to:
```rust
use crate::config::types::memory::{ExpansionConfig, RetrievalScoringConfig};
```

Add a field to the struct (after `scoring: RetrievalScoringConfig,`, ~line 48):
```rust
    /// Associative graph expansion of the candidate pool before rerank.
    /// Default-on; a cold graph cache makes it a no-op.
    expansion: ExpansionConfig,
```

In `new()` (after `scoring: RetrievalScoringConfig::default(),`, ~line 58):
```rust
            expansion: ExpansionConfig::default(),
```

- [ ] **Step 2: Add the builder** (after `with_scoring_config`, ~line 69)

```rust
    /// Attach associative graph-expansion config. `new()` is already on; this
    /// lets callers tune or disable it. `weight` is clamped to `[0,1]`.
    #[must_use]
    pub fn with_expansion_config(mut self, cfg: &ExpansionConfig) -> Self {
        self.expansion = cfg.clone();
        self.expansion.weight = self.expansion.weight.clamp(0.0, 1.0);
        self
    }
```

- [ ] **Step 3: Wire `retrieve()`** — replace the `hybrid_search_notes` block (lines 271–277, from `let results = self` through `let facts: Vec<ScoredFact> = results.iter()...`) with:

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
        }

        let facts: Vec<ScoredFact> = results.iter().map(|r| r.to_scored_fact(agent_id)).collect();
```

(`self.indexer.store()` returns `&Arc<S>`; `.as_ref()` yields `&S`, matching `graph_expand`'s `store: &S`.)

- [ ] **Step 4: Wire `retrieve_multi_agent()`** — in the per-agent loop (lines 352–361), replace:

```rust
        for agent_id in agent_ids {
            let results = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await?;
            for r in results {
                all_results.push(r.to_scored_fact(agent_id));
            }
        }
```
with:
```rust
        for agent_id in agent_ids {
            let mut results = self
                .indexer
                .store()
                .hybrid_search_notes(&embedding, query, agent_id, dim, per_agent_limit)
                .await?;
            if self.expansion.is_active() {
                let peers = expansion::graph_expand(
                    self.indexer.store().as_ref(),
                    agent_id,
                    &results,
                    &self.expansion,
                )
                .await;
                results.extend(peers);
            }
            for r in results {
                all_results.push(r.to_scored_fact(agent_id));
            }
        }
```
(The existing `all_results.truncate(self.fetch_limit(limit))` after the loop already bounds the merged multi-agent pool before rerank.)

- [ ] **Step 5: Write the integration test** — append to the `#[cfg(test)] mod tests` in `mod.rs`. Index two notes; A matches the query via FTS, B does not; materialize edge A→B. With the edge, B surfaces; without it (cold), B is absent. Isolates the expansion effect.

```rust
    #[tokio::test]
    async fn retrieve_surfaces_graph_peer_only_when_materialized() {
        use crate::memory::notes::KnowledgeNote;

        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> =
            Arc::new(SqliteMemoryBackend::new(dir.path()).unwrap());

        // A matches the query token "dreame"; B is unrelated lexically.
        let a = KnowledgeNote {
            title: "alpha".to_string(),
            category: "general".to_string(),
            facts: vec!["dreame brand incident".to_string()],
            content_hash: "h_a".to_string(),
            ..Default::default()
        };
        let b = KnowledgeNote {
            title: "beta".to_string(),
            category: "general".to_string(),
            facts: vec!["unrelated vacuum robotics note".to_string()],
            content_hash: "h_b".to_string(),
            ..Default::default()
        };
        backend.index_note(&a, "default", "general").await.unwrap();
        backend.index_note(&b, "default", "general").await.unwrap();

        let indexer = Arc::new(NoteIndexer::new(dir.path().to_path_buf(), backend.clone()));
        // MockEmbeddingProvider (not Failing): retrieve() must reach
        // hybrid_search_notes + the expansion stage. FailingEmbeddingProvider
        // would divert to text_retrieve, which does NOT run expansion. Notes
        // have no stored vectors, so the vector leg is empty and FTS surfaces A.
        let retrieval =
            NoteFactRetrieval::new(indexer, Arc::new(MockEmbeddingProvider::new(1024, "mock")));

        // Cold cache: B must NOT surface for a query that only matches A.
        let cold = retrieval.retrieve("dreame", "default", 10).await.unwrap();
        assert!(
            cold.iter().all(|f| f.fact.id != "general/beta"),
            "without a materialized edge, the unrelated note must not surface"
        );

        // Materialize A -> B; now B surfaces via associative expansion.
        backend
            .replace_graph_related("default", &[(
                "general/alpha".to_string(),
                "general/beta".to_string(),
                4.0,
            )])
            .await
            .unwrap();
        let warm = retrieval.retrieve("dreame", "default", 10).await.unwrap();
        assert!(
            warm.iter().any(|f| f.fact.id == "general/beta"),
            "with a materialized edge, the graph peer must surface"
        );
    }
```

- [ ] **Step 6: Static audit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
grep -n "expansion::graph_expand\|with_expansion_config\|expansion: ExpansionConfig" src/memory/note_retrieval/mod.rs
```
Expected: field present; builder present; `graph_expand` called in **2** places (`retrieve` + `retrieve_multi_agent`). Confirm both call sites pass `self.indexer.store().as_ref()`.

- [ ] **Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/memory/note_retrieval/mod.rs
git commit --no-verify -m "feat(memory): wire graph expansion into primary retrieval path"
```

---

## Task 5: Config chain A — `MemoryConfig.expansion` → `memory_search` tool

Thread `ExpansionConfig` through the on-demand `memory_search` tool path, mirroring how `retrieval_scoring` is threaded. Depends on Task 2.

**Files:**
- Modify: `src/config/types/memory/mod.rs` (field + Default)
- Modify: `src/builtin_tools/memory_search.rs` (`new_with_config` param + builder + `new()` caller)
- Modify: `src/executor/builtin_registry/builder/constructor.rs` (read config + pass arg)

- [ ] **Step 1: Add the `MemoryConfig.expansion` field** — in `mod.rs`, after the `retrieval_scoring` field (line 65):

```rust
    /// Associative 4-signal graph expansion of the retrieval candidate pool
    /// (default-on; cold cache = no-op). Surfaces notes tied to a hit even
    /// without lexical/semantic overlap.
    #[serde(default)]
    pub expansion: ExpansionConfig,
```

In `impl Default for MemoryConfig` (after `retrieval_scoring: RetrievalScoringConfig::default(),`, ~line 140):
```rust
            expansion: ExpansionConfig::default(),
```

(The only `MemoryConfig` struct literal — `src/memory/retrieval.rs:119` — uses `..MemoryConfig::default()`, so it needs no edit.)

- [ ] **Step 2: Add the param + builder call in `memory_search.rs::new_with_config`**

Change the signature (line 206–212) to add a 6th param:
```rust
    pub fn new_with_config(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        similarity_threshold: Option<f32>,
        rerank_config: Option<&crate::memory::rerank::RerankConfig>,
        scoring_config: Option<&crate::config::types::memory::RetrievalScoringConfig>,
        expansion_config: Option<&crate::config::types::memory::ExpansionConfig>,
    ) -> Self {
```

After the `scoring_config` block (lines 227–229), add:
```rust
        if let Some(cfg) = expansion_config {
            retrieval = retrieval.with_expansion_config(cfg);
        }
```

Update the `new()` caller (line 195):
```rust
        Self::new_with_config(database, embedder, None, None, None, None)
```

- [ ] **Step 3: Update `constructor.rs`** — extend the tuple (lines 315–327) to carry expansion:

Replace:
```rust
            let (rerank_cfg, scoring_cfg): (
                Option<crate::memory::rerank::RerankConfig>,
                Option<crate::config::types::memory::RetrievalScoringConfig>,
            ) = match &config.config {
                Some(cfg) => {
                    let guard = cfg.read().await;
                    (
                        Some(guard.memory.rerank.clone()),
                        Some(guard.memory.retrieval_scoring.clone()),
                    )
                }
                None => (None, None),
            };
```
with:
```rust
            let (rerank_cfg, scoring_cfg, expansion_cfg): (
                Option<crate::memory::rerank::RerankConfig>,
                Option<crate::config::types::memory::RetrievalScoringConfig>,
                Option<crate::config::types::memory::ExpansionConfig>,
            ) = match &config.config {
                Some(cfg) => {
                    let guard = cfg.read().await;
                    (
                        Some(guard.memory.rerank.clone()),
                        Some(guard.memory.retrieval_scoring.clone()),
                        Some(guard.memory.expansion.clone()),
                    )
                }
                None => (None, None, None),
            };
```

Then update the `new_with_config` call (lines 328–334) to pass the new arg:
```rust
            let search_tool = MemorySearchTool::new_with_config(
                db.clone(),
                Arc::clone(embedder),
                config.memory_similarity_threshold,
                rerank_cfg.as_ref(),
                scoring_cfg.as_ref(),
                expansion_cfg.as_ref(),
            );
```

- [ ] **Step 4: Static audit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
echo "--- new_with_config arity (def + 2 callers must all be 6 args) ---"
grep -n "new_with_config" src/builtin_tools/memory_search.rs src/executor/builtin_registry/builder/constructor.rs
echo "--- expansion field present ---"
grep -n "pub expansion: ExpansionConfig\|expansion: ExpansionConfig::default()\|guard.memory.expansion" src/config/types/memory/mod.rs src/executor/builtin_registry/builder/constructor.rs
```
Expected: `new()` passes 6 args (4× `None`); `constructor.rs` passes `expansion_cfg.as_ref()`; field + Default present; `guard.memory.expansion` read once.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/config/types/memory/mod.rs src/builtin_tools/memory_search.rs src/executor/builtin_registry/builder/constructor.rs
git commit --no-verify -m "feat(config): thread ExpansionConfig through memory_search tool"
```

---

## Task 6: Config chain B — `AssemblerConfig.expansion` → proactive memory context

Mirror `expansion` into `AssemblerConfig` (as `retrieval_scoring`/`rerank` already are) and wire the proactive `memory_context_provider` path. Fix the two explicit `AssemblerConfig` struct literals. Depends on Task 2 + Task 4.

**Files:**
- Modify: `src/config/types/memory/assembler.rs` (mirror field + Default)
- Modify: `src/config/types/memory/mod.rs` (`assembler_config()` sync)
- Modify: `src/thinker/memory_context_provider/constructor.rs` (builder call)
- Modify: `src/memory/session_search_summary/filter.rs` (literal fix)
- Modify: `src/memory/assembler/tests.rs` (literal fix)

- [ ] **Step 1: Add the mirror field to `AssemblerConfig`** — in `assembler.rs`, after the `rerank` field (~line 45, the last field before the closing `}`):

```rust

    /// Mirror of `MemoryConfig.expansion`, populated by the server builder so
    /// the proactive memory-context path applies the same associative graph
    /// expansion as the on-demand `memory_search` tool. Default-on; cold cache
    /// = no-op.
    #[serde(default)]
    pub expansion: super::ExpansionConfig,
```

In `impl Default for AssemblerConfig` (after `rerank: crate::memory::rerank::RerankConfig::default(),`, ~line 60):
```rust
            expansion: super::ExpansionConfig::default(),
```

- [ ] **Step 2: Sync in `assembler_config()`** — in `mod.rs`, inside `pub fn assembler_config()` after `cfg.rerank = self.rerank.clone();` (~line 124):

```rust
        cfg.expansion = self.expansion.clone();
```

- [ ] **Step 3: Wire `memory_context_provider`** — in `constructor.rs` (~lines 186–189), add the builder call:

```rust
        let retrieval = Arc::new(
            NoteFactRetrieval::new(indexer, embedder)
                .with_rerank_config(&assembler_config.rerank)
                .with_scoring_config(&assembler_config.retrieval_scoring)
                .with_expansion_config(&assembler_config.expansion),
        );
```

- [ ] **Step 4: Fix the two `AssemblerConfig` struct literals**

`src/memory/session_search_summary/filter.rs` — in the literal (after `rerank: Default::default(),`, ~line 136):
```rust
            expansion: Default::default(),
```

`src/memory/assembler/tests.rs` — in `default_cfg()` (after `rerank: Default::default(),`, ~line 104):
```rust
        expansion: Default::default(),
```

- [ ] **Step 5: Static audit** — every `AssemblerConfig` literal must now list `expansion`; field count parity.

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
echo "--- field + Default + sync ---"
grep -n "pub expansion: super::ExpansionConfig\|expansion: super::ExpansionConfig::default()\|cfg.expansion = self.expansion" src/config/types/memory/assembler.rs src/config/types/memory/mod.rs
echo "--- builder call ---"
grep -n "with_expansion_config" src/thinker/memory_context_provider/constructor.rs
echo "--- both explicit literals carry expansion (expect rerank+expansion adjacent in each) ---"
grep -n "expansion: Default::default()" src/memory/session_search_summary/filter.rs src/memory/assembler/tests.rs
echo "--- confirm NO other AssemblerConfig literal was missed ---"
grep -rn "AssemblerConfig {" src --include="*.rs"
```
Expected: field/Default/sync present; one `with_expansion_config` in the provider; `expansion: Default::default()` in both literal sites; the `AssemblerConfig {` grep lists only `filter.rs` + `assembler/tests.rs` (both now fixed) — no other literal.

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add src/config/types/memory/assembler.rs src/config/types/memory/mod.rs src/thinker/memory_context_provider/constructor.rs src/memory/session_search_summary/filter.rs src/memory/assembler/tests.rs
git commit --no-verify -m "feat(config): thread ExpansionConfig through proactive memory context"
```

---

## Task 7: Documentation

**Files:**
- Modify: `docs/reference/memory/RETRIEVAL.md`

- [ ] **Step 1: Document the expansion stage**

Read `docs/reference/memory/RETRIEVAL.md` §1 (Entry Points) and the scoring-pipeline section (search "RRF" / "scoring"). Insert a new subsection documenting the stage. Place it after the hybrid-fusion description and before the scoring (§4) description, since it sits between them in the pipeline:

```markdown
### Associative Graph Expansion (pre-rerank)

Between `hybrid_search_notes` and the cross-encoder rerank, `retrieve()` runs
`note_retrieval::expansion::graph_expand`. For the top `max_seeds` direct hits it
looks up each seed's strongest 4-signal related peers (`NoteStore::related_peers`,
materialized per dream cycle in `notes_graph_related`), dedups them against the
direct hits, hydrates their content (`NoteStore::get_notes_with_content`), and
adds them to the candidate pool with a propagated score
`seed.score * weight * (edge / seed_top_edge)` — scaled strictly below the seed,
so a peer can displace a *weak* direct hit but never a strong one. This is
associative / multi-hop recall: a note surfaces because it is tied to a
query-relevant note, even without lexical or semantic overlap with the query.

Controlled by `memory.expansion` (`ExpansionConfig`: `enabled`, `max_seeds`,
`peers_per_seed`, `max_expanded`, `weight`). Default-on and conservative. A cold
graph cache (pre-first-dream) makes `related_peers` empty, so expansion is a
no-op and ranking is byte-for-byte legacy; `enabled = false` does the same. Store
errors inside expansion are swallowed (logged) — a graph-cache problem never
fails core recall. The same stage runs per-agent in `retrieve_multi_agent`.
```

Also update the `retrieve` row note or §1 prose if it enumerates pipeline stages, to mention expansion runs before scoring.

- [ ] **Step 2: Static audit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
grep -n "Associative Graph Expansion\|graph_expand\|memory.expansion" docs/reference/memory/RETRIEVAL.md
```
Expected: the new subsection present.

- [ ] **Step 3: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
git add docs/reference/memory/RETRIEVAL.md
git commit --no-verify -m "docs(memory): document graph expansion retrieval stage"
```

---

## Final verification (controller, after all tasks)

No cargo. Static audit of the whole change:

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-graphexp
echo "=== commits on branch ===" && git log --oneline main..HEAD
echo "=== get_notes_with_content: 2 defs (trait + impl) ===" && grep -rc "async fn get_notes_with_content" src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
echo "=== graph_expand call sites: 2 (retrieve + multi_agent) ===" && grep -c "expansion::graph_expand" src/memory/note_retrieval/mod.rs
echo "=== new_with_config arity parity ===" && grep -n "new_with_config" src/builtin_tools/memory_search.rs src/executor/builtin_registry/builder/constructor.rs
echo "=== every AssemblerConfig literal lists expansion ===" && grep -rn "AssemblerConfig {" src --include="*.rs"
echo "=== no stray TODO/unimplemented in new code ===" && grep -rn "todo!\|unimplemented!\|TODO" src/memory/note_retrieval/expansion.rs
```

Expected: 7 commits; `get_notes_with_content` count `1` + `1`; `graph_expand` count `2`; `new_with_config` appears with 6-arg arity at all three sites; both `AssemblerConfig {` literals are the known two; no `todo!`/`TODO` in `expansion.rs`.

**Then STOP.** Do not merge to `main`, do not run cargo. Report branch state to the user for review.

---

## Spec Coverage Check

| Spec section | Task |
|---|---|
| §4.1 `graph_expand` | Task 3 |
| §4.2 `get_notes_with_content` | Task 1 |
| §4.3 `ExpansionConfig` | Task 2 |
| §4.4 field/builder/`retrieve`/`retrieve_multi_agent` | Task 4 |
| §4.5 config plumbing (both chains) | Tasks 5 + 6 |
| §5 data flow (pool cap, merged truncate) | Task 4 (retrieve cap; multi-agent existing truncate) |
| §6 degradation (cold cache, errors swallowed, `enabled=false`) | Task 3 (logic) + Task 2 (`is_active`) + Task 4 (`is_active` gate) |
| §7 tests (11 specified) | Task 1 (1) + Task 2 (2) + Task 3 (7) + Task 4 (1) |
| §8 entropy / no orphan | every new symbol wired (audited in Final verification) |
| §9 file manifest | File Structure table above |
| §10 out of scope (no `community_peers`, no reweight, `gather_related` untouched) | honored — none of those files touched |

# Memory Wiring Fixes + Dead-Code Removal (Gap C) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire three orphaned-but-implemented memory capabilities (`rrf_k`, `bm25_bonus_weight`, signal-driven compression) into the live path, and delete four dead-code clusters — all inside `src/memory/` + 2 call sites.

**Architecture:** Surgical edits only. W1/W2 add a `RetrievalTuning` field to `SqliteMemoryBackend` (defaulting to today's hardcoded 60/0.15) set via a `self`-consuming builder at the single live construction site — no `new()` signature change, so the 40+ test construction sites are untouched. S1 replaces the live turn-counter call site with a signal-aware variant and deletes the parallel unused trigger abstractions. D1/D2/D3 are pure deletions of confirmed-dead code.

**Tech Stack:** Rust, tokio, rusqlite, serde.

---

## ⚠️ Project Protocol (overrides default TDD/verification)

The user's mandatory constraints for THIS project:

1. **Worktree isolation** — all work in branch `fix/memory-wiring-gap-c` off `main`. Never touch `main` directly.
2. **NO `cargo check` / `cargo test` validation after tasks — commit directly.** Because the compiler will NOT catch missed call sites, every deletion task includes **explicit `grep` verification of zero remaining callers** before deleting. This replaces compiler-driven safety. Test code is included as the correctness spec but is NOT run as a gating step.
3. **Append-only on shared `main`** — `main` has concurrent committers. Use **explicit-path `git add`** (never `git add -A`/`-u`). No `reset`/`amend`/`rebase`. Each task = one append commit. The worktree branch is yours alone, but keep the same discipline so the eventual merge is clean.
4. **Entropy reduction** — delete dead code outright; no future-proof stubs, no "reserved" comments.

---

## File Structure

| File | Task | Change |
|---|---|---|
| `src/memory/store/sqlite/mod.rs` | 1 | Add `RetrievalTuning` struct + `tuning` field + `with_retrieval_tuning` builder |
| `src/memory/store/sqlite/notes.rs` | 1 | `hybrid_search_notes` reads `self.tuning` |
| `src/bin/aleph-server/commands/start/mod.rs` | 1 | Chain `.with_retrieval_tuning(...)` at live construction (line ~314) |
| `src/memory/compression/service.rs` | 2 | Add `record_turn_and_check_signal`; delete dead `check_and_compress_with_signal` + `record_turn_and_check` + their tests |
| `src/gateway/execution_engine/execute.rs` | 2 | Rewire call site (line 512) |
| `src/memory/compression/signal_detector.rs` | 3 | Delete `ContextSwitch` variant + `detect_context_switch` + `cosine_distance` + `detect_with_context` + 3 tests |
| `src/memory/compression/trigger.rs` | 3 | **Delete entire file** |
| `src/memory/compression/mod.rs` | 3 | Remove `mod trigger;` + `pub use trigger::{...}` |
| `src/memory/events/handler.rs` | 4 | Remove `memory_store` field + `new()` param + unused import |
| `src/bin/aleph-server/commands/start/builder/handlers/memory.rs` | 4 | Drop `Some(memory_db…)` arg |
| `src/executor/builtin_registry/builder/constructor.rs` | 4 | Drop `Some(db…)` arg |
| `src/memory/notes/wikilink.rs` | 5 | Delete `resolve_wikilink` + its tests + unused import |
| `src/memory/notes/mod.rs` | 5 | Remove `resolve_wikilink` from re-export |
| `src/memory/assembler/rerank.rs` | 6 | Delete `reasoning` field + `#[allow(dead_code)]` |

---

## Task 0: Create the worktree

- [ ] **Step 1: Create isolated worktree branch**

REQUIRED SUB-SKILL: Use superpowers:using-git-worktrees to create a worktree on branch `fix/memory-wiring-gap-c` off `main`.

> ⚠️ Per CLAUDE.md: do NOT `git worktree remove` inside the same session that used `EnterWorktree` — it permanently corrupts the shell. Merge in this session; clean up the worktree in a fresh session.

Expected: a clean worktree at branch `fix/memory-wiring-gap-c`, HEAD = current `main`.

---

## Task 1: W1 + W2 — wire `rrf_k` and `bm25_bonus_weight`

**Files:**
- Modify: `src/memory/store/sqlite/mod.rs` (struct at line 26-28, `new()` at 42-84, `in_memory()` at 96-109)
- Modify: `src/memory/store/sqlite/notes.rs:653-707` (`hybrid_search_notes`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:308-319` (live construction)

- [ ] **Step 1: Add `RetrievalTuning` struct + field to the backend**

In `src/memory/store/sqlite/mod.rs`, after the imports (around line 23) add the struct, and add the field to `SqliteMemoryBackend`:

```rust
/// Tunable knobs for hybrid (vector + FTS) retrieval fusion.
///
/// Defaults reproduce the historical hardcoded behaviour (RRF k=60,
/// BM25 lift 0.15) so every construction site that does not override
/// these is byte-for-byte unchanged.
#[derive(Debug, Clone, Copy)]
pub struct RetrievalTuning {
    /// Reciprocal Rank Fusion constant.
    pub rrf_k: u32,
    /// Extra multiplicative lift applied to FTS (lexical) matches in fusion.
    pub bm25_bonus_weight: f32,
}

impl Default for RetrievalTuning {
    fn default() -> Self {
        Self {
            rrf_k: 60,
            bm25_bonus_weight: 0.15,
        }
    }
}
```

Change the struct (lines 26-28) from:

```rust
pub struct SqliteMemoryBackend {
    conn: Mutex<Connection>,
}
```

to:

```rust
pub struct SqliteMemoryBackend {
    conn: Mutex<Connection>,
    tuning: RetrievalTuning,
}
```

- [ ] **Step 2: Initialise the field in `new()` and `in_memory()`, add the builder**

In `new()` change the returned `Ok(Self { conn: Mutex::new(conn) })` (lines 81-83) to:

```rust
        Ok(Self {
            conn: Mutex::new(conn),
            tuning: RetrievalTuning::default(),
        })
```

In `in_memory()` change `Ok(Self { conn: Mutex::new(conn) })` (lines 106-108) the same way:

```rust
        Ok(Self {
            conn: Mutex::new(conn),
            tuning: RetrievalTuning::default(),
        })
```

Immediately after `in_memory()` (after line 109), add the builder:

```rust
    /// Override retrieval fusion tuning. Consumes and returns `self` so it
    /// can be chained right after `new()` at the live construction site,
    /// before the backend is wrapped in `Arc` and shared.
    pub fn with_retrieval_tuning(mut self, rrf_k: u32, bm25_bonus_weight: f32) -> Self {
        self.tuning = RetrievalTuning {
            rrf_k,
            bm25_bonus_weight,
        };
        self
    }
```

- [ ] **Step 3: Read the tuning in `hybrid_search_notes`**

In `src/memory/store/sqlite/notes.rs`, replace the fusion block (lines 670-682):

```rust
        // RRF fusion with k=60 (standard)
        let k = 60.0_f32;
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (rank, (path, _score)) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(path.clone()).or_insert(0.0) += rrf;
        }

        for (rank, entry) in fts_entries.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(entry.path.clone()).or_insert(0.0) += rrf;
        }
```

with (reads `self.tuning`; applies the BM25 lift to FTS-matched entries):

```rust
        // RRF fusion. `rrf_k` is the standard Reciprocal Rank Fusion
        // constant; FTS (lexical) matches get an extra `bm25_bonus_weight`
        // multiplicative lift so operators can bias toward keyword hits.
        let k = self.tuning.rrf_k as f32;
        let bm25_lift = 1.0 + self.tuning.bm25_bonus_weight;
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (rank, (path, _score)) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(path.clone()).or_insert(0.0) += rrf;
        }

        for (rank, entry) in fts_entries.iter().enumerate() {
            let rrf = (1.0 / (k + (rank as f32) + 1.0)) * bm25_lift;
            *scores.entry(entry.path.clone()).or_insert(0.0) += rrf;
        }
```

- [ ] **Step 4: Wire the live construction site**

In `src/bin/aleph-server/commands/start/mod.rs`, the live backend is built at lines 308-319. Change the `Ok(backend)` arm (lines 310-315) from:

```rust
            Ok(backend) => {
                if !args.daemon {
                    println!("Memory backend initialized (SQLite + sqlite-vec)");
                }
                Arc::new(backend)
            }
```

to:

```rust
            Ok(backend) => {
                if !args.daemon {
                    println!("Memory backend initialized (SQLite + sqlite-vec)");
                }
                Arc::new(backend.with_retrieval_tuning(
                    loaded_app_config.memory.rrf_k,
                    loaded_app_config.memory.bm25_bonus_weight,
                ))
            }
```

> Verify the field path first: `grep -n "pub memory" src/config/types/*.rs src/config/**/*.rs` should show `loaded_app_config` carries a `memory: MemoryConfig`. The fields `rrf_k: u32` and `bm25_bonus_weight: f32` are confirmed at `src/config/types/memory/mod.rs:75,77`.

- [ ] **Step 5: Add a focused unit test (correctness spec)**

Append to the existing `#[cfg(test)] mod tests` in `src/memory/store/sqlite/notes.rs` (or create one if absent). This documents that the knob changes fusion; it is the spec, not a gating run:

```rust
    #[test]
    fn retrieval_tuning_default_matches_legacy_constants() {
        let t = crate::memory::store::sqlite::RetrievalTuning::default();
        assert_eq!(t.rrf_k, 60);
        assert_eq!(t.bm25_bonus_weight, 0.15);
    }

    #[test]
    fn with_retrieval_tuning_overrides_fields() {
        let backend = crate::memory::store::sqlite::SqliteMemoryBackend::in_memory()
            .unwrap()
            .with_retrieval_tuning(42, 0.5);
        assert_eq!(backend.tuning.rrf_k, 42);
        assert_eq!(backend.tuning.bm25_bonus_weight, 0.5);
    }
```

- [ ] **Step 6: Commit (per protocol — no cargo check)**

```bash
git add src/memory/store/sqlite/mod.rs src/memory/store/sqlite/notes.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "memory: wire rrf_k + bm25_bonus_weight config into hybrid fusion"
```

---

## Task 2: S1-3a — wire signal-driven compression into the live turn path

**Files:**
- Modify: `src/memory/compression/service.rs` (add method; delete dead `check_and_compress_with_signal` lines 439-474 + `record_turn_and_check` lines 575-612 + their tests)
- Modify: `src/gateway/execution_engine/execute.rs:510-513`

- [ ] **Step 1: Add the signal-aware turn handler**

In `src/memory/compression/service.rs`, add this method to `impl CompressionService` (place it right after the existing `record_turn_and_check` at line 612, before `get_scheduler`):

```rust
    /// Record a conversation turn and trigger compression — signal-aware.
    ///
    /// Always counts the turn with exactly-once threshold-crossing semantics
    /// (so the turn-threshold path keeps working). Additionally, if the user
    /// message carries an `Immediate` signal (a correction like "不对/错了/
    /// wrong"), compress NOW instead of waiting for the threshold. Learning
    /// and milestone signals ride the normal turn-threshold cadence.
    ///
    /// Non-blocking: the actual compression runs in a spawned task.
    pub fn record_turn_and_check_signal(self: &Arc<Self>, user_message: &str) {
        let detection = self.signal_detector.detect(user_message);

        // Count the turn exactly once at the threshold crossing.
        let old_turns = self
            .scheduler
            .pending_turns
            .fetch_add(1, crate::sync_primitives::Ordering::AcqRel);
        let turns = old_turns + 1;
        let threshold = self.config.scheduler.turn_threshold;
        let threshold_crossed = old_turns < threshold && turns >= threshold;

        let immediate = detection.should_compress
            && detection.priority == super::signal_detector::CompressionPriority::Immediate;

        if immediate {
            tracing::info!(signals = ?detection.signals, "Signal-triggered compression (immediate)");
            let service = Arc::clone(self);
            tokio::spawn(async move {
                match service.compress().await {
                    Ok(result) => tracing::info!(
                        facts = result.facts_extracted,
                        "Immediate compression completed (signal)"
                    ),
                    Err(e) => tracing::error!(error = %e, "Immediate compression failed (signal)"),
                }
            });
        } else if threshold_crossed {
            tracing::info!(turns, threshold, "Turn threshold reached, triggering compression");
            let service = Arc::clone(self);
            tokio::spawn(async move {
                match service.check_and_compress().await {
                    Ok(Some(result)) => tracing::info!(
                        facts = result.facts_extracted,
                        "Immediate compression completed (turn threshold)"
                    ),
                    Ok(None) => tracing::debug!("Compression: no action needed"),
                    Err(e) => tracing::error!(error = %e, "Compression failed (turn threshold)"),
                }
            });
        }
    }
```

- [ ] **Step 2: Rewire the gateway call site**

In `src/gateway/execution_engine/execute.rs`, change lines 510-513:

```rust
                // Record conversation turn for compression scheduling
                if let Some(ref cs) = self.compression_service {
                    cs.record_turn_and_check();
                }
```

to:

```rust
                // Record conversation turn for compression scheduling.
                // Signal-aware: corrections compress immediately; other turns
                // ride the turn-threshold cadence.
                if let Some(ref cs) = self.compression_service {
                    cs.record_turn_and_check_signal(&request.input);
                }
```

> `request.input` is the user message; it is already cloned as `ui` at line 504 for the memory write, confirming it is in scope and a `String`/`&str`.

- [ ] **Step 3: Delete the now-dead superseded methods + their tests**

First confirm zero remaining callers (no compiler to catch misses):

```bash
grep -rn "check_and_compress_with_signal\|record_turn_and_check\b" src/ | grep -v "record_turn_and_check_signal"
```

Expected: only the definitions in `service.rs` and their `#[cfg(test)]` tests — NO production callers (the gateway now uses `record_turn_and_check_signal`).

Then in `src/memory/compression/service.rs`:
- Delete `check_and_compress_with_signal` (lines 439-474, the whole `pub async fn … }`).
- Delete `record_turn_and_check` (lines 575-612, the whole `pub fn … }`).
- In the `#[cfg(test)] mod tests`, delete the test `test_signal_triggered_compression` (the test that calls `check_and_compress_with_signal`) and any test that calls `record_turn_and_check` directly. Locate them with:

```bash
grep -n "check_and_compress_with_signal\|record_turn_and_check\b" src/memory/compression/service.rs
```

Delete each matching test function in full.

- [ ] **Step 4: Add a unit test for the new handler (correctness spec)**

Add to `src/memory/compression/service.rs` `#[cfg(test)] mod tests`. This asserts a correction message yields an `Immediate` signal classification (the trigger condition); it does not depend on async compression actually running:

```rust
    #[test]
    fn correction_message_classifies_immediate() {
        let detector = crate::memory::compression::signal_detector::SignalDetector::new();
        let d = detector.detect("不对，我说的是 Rust");
        assert!(d.should_compress);
        assert_eq!(
            d.priority,
            crate::memory::compression::signal_detector::CompressionPriority::Immediate
        );
    }

    #[test]
    fn neutral_message_does_not_force_compression() {
        let detector = crate::memory::compression::signal_detector::SignalDetector::new();
        let d = detector.detect("帮我看一下这段代码");
        assert!(!d.should_compress);
    }
```

- [ ] **Step 5: Commit**

```bash
git add src/memory/compression/service.rs src/gateway/execution_engine/execute.rs
git commit -m "memory: wire signal-driven compression into live turn path"
```

---

## Task 3: S1-3b — delete the parallel unused trigger abstractions

**Files:**
- Modify: `src/memory/compression/signal_detector.rs`
- Delete: `src/memory/compression/trigger.rs`
- Modify: `src/memory/compression/mod.rs`

- [ ] **Step 1: Confirm nothing live uses the deletion targets**

```bash
grep -rn "HybridTrigger\|TriggerReason\|TriggerConfig\|CompressionAggressiveness\|detect_with_context\|detect_context_switch\|ContextSwitch\|cosine_distance" src/ | grep -v "src/memory/compression/trigger.rs\|src/memory/compression/signal_detector.rs\|src/memory/compression/mod.rs"
```

Expected: **no output** (all references are confined to the three files being edited/deleted). If anything else appears, STOP and report — the deletion is not self-contained.

- [ ] **Step 2: Delete `trigger.rs` entirely**

```bash
git rm src/memory/compression/trigger.rs
```

- [ ] **Step 3: Remove trigger from `mod.rs`**

In `src/memory/compression/mod.rs`, delete line 16 (`mod trigger;`) and line 23 (`pub use trigger::{CompressionAggressiveness, HybridTrigger, TriggerConfig, TriggerReason};`). The file's `pub use` block becomes:

```rust
pub use scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
pub use service::{CompressionConfig, CompressionService, PostCompressionHook};
pub use signal_detector::{
    CompressionPriority, CompressionSignal, DetectionResult, SignalDetector, SignalKeywords,
};
```

- [ ] **Step 4: Delete the `ContextSwitch` variant**

In `src/memory/compression/signal_detector.rs`, delete the `ContextSwitch` variant from the `CompressionSignal` enum (lines 35-39):

```rust
    /// User is switching context/topic
    ContextSwitch {
        from_topic: String,
        to_topic: String,
    },
```

- [ ] **Step 5: Delete the embedding-based detection methods**

In `src/memory/compression/signal_detector.rs`, delete these three methods in full (they form a contiguous block from ~line 231 to ~308):
- `detect_context_switch` (doc comment starts ~line 231, `pub fn detect_context_switch(` ~line 239, through its closing `}`)
- `cosine_distance` (private helper, `fn cosine_distance(` ~line 267, through its closing `}`)
- `detect_with_context` (doc comment ~line 280, `pub fn detect_with_context(` ~line 284, through its closing `}` ~line 307)

Stop at the `impl Default for SignalDetector` block (line 310) — that stays. The `PostCompactCleanup` impl (line 320+) also stays.

- [ ] **Step 6: Delete the three orphaned tests**

In the `#[cfg(test)] mod tests` of `src/memory/compression/signal_detector.rs`, delete these test functions in full:
- `test_context_switch_detection`
- `test_no_context_switch_for_similar_topics`
- `test_detect_with_context_combines_signals`

Confirm none remain:

```bash
grep -n "ContextSwitch\|detect_with_context\|detect_context_switch\|cosine_distance" src/memory/compression/signal_detector.rs
```

Expected: **no output**.

- [ ] **Step 7: Commit**

```bash
git add src/memory/compression/signal_detector.rs src/memory/compression/mod.rs
git commit -m "memory: remove unused HybridTrigger + context-switch detection (dead code)"
```

---

## Task 4: D1 — remove dead `memory_store` field

**Files:**
- Modify: `src/memory/events/handler.rs` (field 25-26, `new()` 33-39, import 17)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/memory.rs:286-289`
- Modify: `src/executor/builtin_registry/builder/constructor.rs:1499-1502`

- [ ] **Step 1: Remove the field and constructor param**

In `src/memory/events/handler.rs`:

Change the struct (lines 23-30) from:

```rust
pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,
    #[allow(dead_code)] // reserved: memory backend handle, not yet consumed
    memory_store: Option<MemoryBackend>,
    /// NoteIndexer for the notes write path.
    /// When present, every create/update/delete also writes to the notes filesystem layer.
    note_indexer: Option<Arc<NoteIndexer<SqliteMemoryBackend>>>,
}
```

to:

```rust
pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,
    /// NoteIndexer for the notes write path.
    /// When present, every create/update/delete also writes to the notes filesystem layer.
    note_indexer: Option<Arc<NoteIndexer<SqliteMemoryBackend>>>,
}
```

Change `new()` (lines 33-39) from:

```rust
    pub fn new(db: Arc<StateDatabase>, memory_store: Option<MemoryBackend>) -> Self {
        Self {
            db,
            memory_store,
            note_indexer: None,
        }
    }
```

to:

```rust
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self {
            db,
            note_indexer: None,
        }
    }
```

- [ ] **Step 2: Remove the now-unused import**

In `src/memory/events/handler.rs`, line 17 is `use crate::memory::store::MemoryBackend;`. Confirm `MemoryBackend` is not used elsewhere in the file:

```bash
grep -n "MemoryBackend" src/memory/events/handler.rs
```

If the only hit is the `use` line, delete line 17. (`SqliteMemoryBackend` on line 16 is a different import and stays.)

- [ ] **Step 3: Update the three call sites**

`src/memory/events/handler.rs:412` (test): change `MemoryCommandHandler::new(db, None)` → `MemoryCommandHandler::new(db)`.

`src/bin/aleph-server/commands/start/builder/handlers/memory.rs:286-289`: change

```rust
    std::sync::Arc::new(MemoryCommandHandler::new(
        std::sync::Arc::clone(state_db),
        Some(memory_db.clone()),
    ))
```

to:

```rust
    std::sync::Arc::new(MemoryCommandHandler::new(std::sync::Arc::clone(state_db)))
```

Then check whether `memory_db` is still used elsewhere in that function:

```bash
grep -n "memory_db" src/bin/aleph-server/commands/start/builder/handlers/memory.rs
```

If this was its only use, prefix the parameter with `_` (e.g. `_memory_db`) at its declaration to silence the unused-variable lint; if still used elsewhere, leave it.

`src/executor/builtin_registry/builder/constructor.rs:1499-1502`: change

```rust
                let handler = Arc::new(crate::memory::events::handler::MemoryCommandHandler::new(
                    Arc::clone(state_db),
                    Some(db.clone()),
                ));
```

to:

```rust
                let handler = Arc::new(crate::memory::events::handler::MemoryCommandHandler::new(
                    Arc::clone(state_db),
                ));
```

(Here `db` remains used elsewhere — it backs `note_indexer`/tool wiring — so no `_` change needed. Confirm with `grep -n "db\." src/executor/builtin_registry/builder/constructor.rs | sed -n '1,5p'` if unsure.)

- [ ] **Step 4: Final caller sweep**

```bash
grep -rn "MemoryCommandHandler::new" src/
```

Expected: every hit now passes a single argument. No `Some(` / `None` second arg remains.

- [ ] **Step 5: Commit**

```bash
git add src/memory/events/handler.rs src/bin/aleph-server/commands/start/builder/handlers/memory.rs src/executor/builtin_registry/builder/constructor.rs
git commit -m "memory: remove dead memory_store field from MemoryCommandHandler"
```

---

## Task 5: D2 — delete orphan `resolve_wikilink`

**Files:**
- Modify: `src/memory/notes/wikilink.rs` (function 76-98, import 7, its tests)
- Modify: `src/memory/notes/mod.rs:28`

- [ ] **Step 1: Confirm `resolve_wikilink` has no live caller**

```bash
grep -rn "resolve_wikilink" src/ | grep -v "wikilink.rs"
```

Expected: only `src/memory/notes/mod.rs:28` (the re-export). No production caller.

- [ ] **Step 2: Remove from the re-export**

In `src/memory/notes/mod.rs:28`, change:

```rust
pub use wikilink::{extract_wikilinks, remove_wikilink, resolve_wikilink, rewrite_wikilinks};
```

to:

```rust
pub use wikilink::{extract_wikilinks, remove_wikilink, rewrite_wikilinks};
```

- [ ] **Step 3: Delete the function**

In `src/memory/notes/wikilink.rs`, delete `resolve_wikilink` in full (the doc comment at lines 76-80 plus `pub async fn resolve_wikilink<S: NoteStore>(` lines 81-98, through its closing `}`).

- [ ] **Step 4: Delete its tests + now-dead test helpers**

In the `#[cfg(test)] mod tests`, delete every test that calls `resolve_wikilink` (async tests resolving links). Locate them:

```bash
grep -n "resolve_wikilink\|find_by_filename\|fn make_\|SqliteMemoryBackend" src/memory/notes/wikilink.rs
```

Delete the `resolve_wikilink` test functions. If a test-helper constructor (e.g. the `SqliteMemoryBackend::new(...)` builder at line ~192) becomes unused after removing those tests, delete it too. Keep the pure-string tests (`extracts_wikilinks_from_text`, `rewrites_wikilinks`, etc.) — they exercise the surviving functions.

- [ ] **Step 5: Remove the now-unused `NoteStore` import**

`resolve_wikilink` was the only user of the `NoteStore` trait in this file. Confirm:

```bash
grep -n "NoteStore" src/memory/notes/wikilink.rs
```

If the only remaining hit is `use crate::memory::notes::store::NoteStore;` (line 7), delete line 7. If a surviving test still references it, leave it.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/wikilink.rs src/memory/notes/mod.rs
git commit -m "memory: delete orphan resolve_wikilink (superseded by notes_links index)"
```

---

## Task 6: D3 — delete dead `RerankResponse::reasoning`

**Files:**
- Modify: `src/memory/assembler/rerank.rs:78-83`

- [ ] **Step 1: Confirm `reasoning` is never read**

```bash
grep -rn "\.reasoning\b" src/memory/assembler/
```

Expected: no output (the field is parsed by serde but never accessed).

- [ ] **Step 2: Delete the field**

In `src/memory/assembler/rerank.rs`, delete lines 82-83:

```rust
    #[allow(dead_code)]
    pub reasoning: Option<String>,
```

The `RerankResponse` struct keeps its remaining fields (e.g. `slots`). serde silently ignores the `"reasoning"` key if the LLM still returns it, so the prompt example at line 25 needs no change.

- [ ] **Step 3: Commit**

```bash
git add src/memory/assembler/rerank.rs
git commit -m "memory: remove parsed-but-unused RerankResponse::reasoning field"
```

---

## Final: integration sweep (no cargo check — grep only)

- [ ] **Step 1: Confirm all deletion targets are gone repo-wide**

```bash
grep -rn "HybridTrigger\|TriggerReason\|CompressionAggressiveness\|detect_with_context\|detect_context_switch\|ContextSwitch\|resolve_wikilink\|check_and_compress_with_signal" src/
```

Expected: **no output**.

- [ ] **Step 2: Confirm the wiring landed**

```bash
grep -rn "record_turn_and_check_signal\|with_retrieval_tuning\|self.tuning" src/
```

Expected: `record_turn_and_check_signal` at its definition + the gateway call site; `with_retrieval_tuning` at its definition + `start/mod.rs`; `self.tuning` in `notes.rs`.

- [ ] **Step 3: Report**

Summarise the 6 commits on `fix/memory-wiring-gap-c`. Per protocol, do NOT run `cargo check`/`cargo test`. Hand back to the user for the merge decision (worktree cleanup happens in a fresh session per CLAUDE.md).

---

## Self-Review

- **Spec coverage:** W1 (Task 1) ✓, W2 (Task 1) ✓, S1-3a wire (Task 2) ✓, S1-3b delete (Task 3) ✓, D1 (Task 4) ✓, D2 (Task 5) ✓, D3 (Task 6) ✓. Spec §6 "not doing" items (C1, NoteMetadataUpdated, X1) correctly have no tasks.
- **Type consistency:** `RetrievalTuning { rrf_k: u32, bm25_bonus_weight: f32 }` used identically in Task 1 struct, builder, and `notes.rs` reads. `record_turn_and_check_signal(self: &Arc<Self>, &str)` signature matches its call site. `CompressionPriority::Immediate` path matches the enum at `signal_detector.rs:44`.
- **No placeholders:** every code step shows full before/after; every deletion step carries a `grep` guard (compiler substitute mandated by the no-`cargo check` protocol).

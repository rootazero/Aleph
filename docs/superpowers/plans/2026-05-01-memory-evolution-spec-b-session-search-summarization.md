# Spec B — Session Search Summarization Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `session_search`'s raw FTS5 hit response with a summary-driven response that returns one synthesized excerpt per matched session plus 0-2 raw evidence quotes; produce session summaries via three coordinated paths (existing compactor, new `on_session_end` fallback, new lazy on-read fallback).

**Architecture:** New module `src/memory/session_search_summary/` owns end-hook glue, lazy synthesizer, per-session dedup, and summary fact lookup. `WorkingMemoryAssembler::assemble` gains an additive `FactSourceFilter` parameter; only the new tool path passes a non-default filter. Summaries land in the existing `memory_facts` table at `aleph://session/{sid}/end-summary` with `INSERT OR IGNORE` (first writer wins). The wiki/note layer (`fact_source != SessionCompressed`) is read-only and untouched.

**Tech Stack:** Rust 1.x, Tokio async, SQLite via `sqlx`-style backend, `schemars` for tool JSON schemas, existing `build_summary_prompt` LLM-prompting helpers, existing `PostCompressionHook` + `register_session_end_mcp` patterns from Spec A.

**Spec reference:** `docs/superpowers/specs/2026-05-01-memory-evolution-spec-b-session-search-summarization-design.md`

**Constraint inherited from Spec A:** the following 5 files are pre-existing dirty and must remain untouched throughout this plan: `interfaces/webchat/dist/aleph_panel.js`, `interfaces/webchat/dist/aleph_panel_bg.wasm`, `src/agents/runtime.rs`, `src/gateway/execution_engine/engine.rs`, `src/gateway/execution_engine/run_loop.rs`. None of Spec B's tasks need to touch these files.

---

## File Structure

### Files to create

| Path | Responsibility |
|---|---|
| `src/memory/session_search_summary/mod.rs` | Module root, re-exports |
| `src/memory/session_search_summary/lookup.rs` | `retrieve_summary_fact(store, agent_id, session_id) -> Option<MemoryFact>` — single-fact lookup by canonical path |
| `src/memory/session_search_summary/dedup.rs` | `top_per_session(candidates, max_sessions) -> Vec<Candidate>` — group + best-score selection |
| `src/memory/session_search_summary/synthesizer.rs` | `SummarySynthesizer::lazy_for(session_id, agent_id) -> Result<MemoryFact>` — windowed transcript load + LLM call + `INSERT OR IGNORE` writeback |
| `src/memory/session_search_summary/end_hook.rs` | `SessionEndSummarizer::produce(session_id, agent_id)` — short-circuit + d* fact reuse + LLM fallback, registered on `on_session_end` |
| `src/memory/session_search_summary/filter.rs` | `FactSourceFilter` enum (could also live in `assembler/`; we put it here to colocate with the only non-default consumer) |
| `tests/spec_b_e2e.rs` | Six end-to-end integration tests (acceptance criteria 1-9) |

### Files to modify

| Path | Change |
|---|---|
| `src/memory/mod.rs` | `pub mod session_search_summary;` |
| `src/memory/assembler/mod.rs` | Add `filter: FactSourceFilter` parameter to `WorkingMemoryAssembler::assemble` trait method; default existing callers via a `_with_filter` overload OR add the parameter directly with `FactSourceFilter::Any` as the default |
| `src/memory/assembler/hybrid.rs` | Implement filter handling in `HybridAssembler::assemble`, threading through `Gatherer` to backend FTS5/vector queries with a `WHERE fact_source = ?` predicate |
| `src/memory/assembler/gather.rs` | Backend candidate query gains a `fact_source` filter clause |
| `src/builtin_tools/session_search.rs` | Full rewrite of `call_impl`: new schema (`summary` / `evidence_quotes` / `source` / drop `content` & `role`), call HybridAssembler with `Only(SessionCompressed)`, dedup, evidence lookup, lazy fallback |
| `src/config/agent_resolver.rs` | Update `session_search` mention in `default_agents` system prompt to teach the LLM the new schema |
| `src/gateway/session_store/mod.rs` | (Optional, if not already supported) Add `session_filter: Option<&str>` parameter to `SessionStore::search_messages`. If not added, the tool post-filters by session_key in Rust |
| `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md` | Mark Spec B row `✅ shipped` after Task 23 lands |
| `docs/reference/memory/RETRIEVAL.md` | Brief subsection on session-summary retrieval path (1 paragraph) |
| `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md` | Index entry for Spec B shipped state |
| `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_b_session_search_summarization.md` | New memory file tracking shipped commits + acceptance evidence |

---

### Task 1: Discovery / API audit

**Purpose:** Lock down exact API surfaces before code lands. No code edits in this task — only `grep` runs and a written audit report committed as a comment in `src/memory/session_search_summary/mod.rs`.

**Files:**
- Create: `src/memory/session_search_summary/mod.rs` (with audit notes as module doc comment + `pub mod` declarations to be filled by later tasks)

- [ ] **Step 1: Audit `WorkingMemoryAssembler::assemble` signature**

Run:
```bash
grep -A 8 "trait WorkingMemoryAssembler" /Volumes/TBU4/Workspace/Aleph/src/memory/assembler/mod.rs
grep -A 30 "impl WorkingMemoryAssembler for HybridAssembler" /Volumes/TBU4/Workspace/Aleph/src/memory/assembler/hybrid.rs
```

Expected: confirm signature `assemble(&self, query: &str, agent_id: &str, session_id: Option<&str>, budget: AssemblyBudget) -> Result<MemoryEnvelope, AlephError>`. Record any deviation.

- [ ] **Step 2: Audit FactSource enum variants**

Run:
```bash
sed -n '175,210p' /Volumes/TBU4/Workspace/Aleph/src/memory/context/enums.rs
```

Expected: confirm `SessionCompressed` is one of the variants. Record the full variant list — Task 2's filter enum will need to handle each.

- [ ] **Step 3: Audit `SessionStore::search_messages` signature**

Run:
```bash
sed -n '30,55p' /Volumes/TBU4/Workspace/Aleph/src/gateway/session_store/mod.rs
```

Expected: confirm whether `search_messages` accepts a `session_id` filter. Record the answer — Task 12 picks between adding the parameter vs post-filtering in Rust.

- [ ] **Step 4: Audit memory_facts write API**

Run:
```bash
grep -rn "fn write_fact\|fn upsert_fact\|fn insert_fact" /Volumes/TBU4/Workspace/Aleph/src/memory/store --include="*.rs" | head -10
grep -rn "MemoryFact" /Volumes/TBU4/Workspace/Aleph/src/memory/store/sqlite --include="*.rs" | grep -i "insert\|write\|upsert" | head -10
```

Expected: locate the canonical write entry-point used by `summary_to_fact` consumers. Record its name + signature. The lazy + session_end paths will both call this. If `INSERT OR IGNORE` is not the default, record what flag/option enables it.

- [ ] **Step 5: Audit `register_session_end_mcp` pattern (Spec A)**

Run:
```bash
sed -n '660,700p' /Volumes/TBU4/Workspace/Aleph/src/thinker/memory_context_provider.rs
grep -n "session_end_mcp\|SESSION_END_MCP" /Volumes/TBU4/Workspace/Aleph/src/gateway/session_manager/ops.rs
```

Expected: confirm the process-wide `OnceCell<Arc<MemoryContextProvider>>` slot pattern. Record whether the slot accepts a single registration or a vec. Task 10 will either reuse it or add a parallel slot.

- [ ] **Step 6: Audit `PostCompressionHook` trait**

Run:
```bash
sed -n '25,55p' /Volumes/TBU4/Workspace/Aleph/src/memory/compression/service.rs
```

Expected: confirm trait shape: `fn on_compression_complete<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, ()>`. We do NOT need to implement this trait in Spec B — recorded for context only.

- [ ] **Step 7: Write audit report as `mod.rs` doc comment**

Create `src/memory/session_search_summary/mod.rs` with content:

```rust
//! Spec B — Session search summarization pipeline.
//!
//! ## Audit findings (Task 1, 2026-05-01)
//!
//! - `WorkingMemoryAssembler::assemble` lives in `src/memory/assembler/mod.rs`
//!   and `HybridAssembler::assemble` in `src/memory/assembler/hybrid.rs`.
//!   Signature: `(query, agent_id, session_id, budget) -> Result<MemoryEnvelope, AlephError>`.
//!   Adding a `filter: FactSourceFilter` parameter is the chosen extension.
//!
//! - `FactSource` enum at `src/memory/context/enums.rs:180`. `SessionCompressed`
//!   is the variant Spec B targets via `FactSourceFilter::Only(SessionCompressed)`.
//!
//! - `SessionStore::search_messages` at `src/gateway/session_store/mod.rs:37`.
//!   See Task 1 step 3 for whether a session-key filter is supported. Task 12
//!   picks between adding the parameter and post-filtering.
//!
//! - Memory-fact write entry-point: see Task 1 step 4 audit notes.
//!
//! - Spec A's `register_session_end_mcp` pattern at
//!   `src/thinker/memory_context_provider.rs:668`. Task 10 either reuses the
//!   slot via a multi-handler vec or adds a parallel slot
//!   `register_session_end_summarizer`.
//!
//! - `PostCompressionHook` exists for compression-time hooks but Spec B does
//!   not implement it; only Spec A consumes it. Recorded for context.

pub mod dedup;
pub mod end_hook;
pub mod filter;
pub mod lookup;
pub mod synthesizer;
```

Replace the audit notes' "see Task 1 step N audit notes" placeholders with the actual findings observed during Steps 1-6.

- [ ] **Step 8: Add the module declaration**

Edit `src/memory/mod.rs`. Find the existing `pub mod` block and add (alphabetical placement near `session_compactor`):

```rust
pub mod session_search_summary;
```

- [ ] **Step 9: Verify the empty module compiles**

Run:
```bash
cargo check -p alephcore 2>&1 | tail -20
```

Expected: clean compile. The 5 sub-modules don't exist yet, but `mod.rs` declares them as `pub mod` — this means each task that creates a sub-module file automatically completes the declaration. To avoid intermediate-state failures, **add empty stub files for each sub-module in this task**:

```bash
for f in dedup end_hook filter lookup synthesizer; do
  echo "//! Spec B — placeholder. See task plan." > /Volumes/TBU4/Workspace/Aleph/src/memory/session_search_summary/${f}.rs
done
```

Re-run `cargo check -p alephcore`. Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/memory/session_search_summary/ src/memory/mod.rs
git commit -m "spec-b: scaffold session_search_summary module + audit findings"
```

---

### Task 2: Add `FactSourceFilter` enum

**Files:**
- Modify: `src/memory/session_search_summary/filter.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Edit `src/memory/session_search_summary/filter.rs`:

```rust
//! Spec B — filter for restricting HybridAssembler results by `FactSource`.
//!
//! Default behaviour (`Any`) is byte-for-byte identical to pre-Spec-B
//! assembler output. `Only(_)` and `Excluding(_)` are non-default values
//! used by `session_search` (and only `session_search`) to physically
//! separate session summaries from wiki/note retrieval.

use crate::memory::context::enums::FactSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactSourceFilter {
    Any,
    Only(FactSource),
    Excluding(FactSource),
}

impl Default for FactSourceFilter {
    fn default() -> Self {
        Self::Any
    }
}

impl FactSourceFilter {
    /// Predicate evaluated row-by-row when filtering candidate facts.
    pub fn matches(&self, source: FactSource) -> bool {
        match self {
            Self::Any => true,
            Self::Only(want) => *want == source,
            Self::Excluding(skip) => *skip != source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_matches_everything() {
        let f = FactSourceFilter::Any;
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::LLMExtracted));
    }

    #[test]
    fn only_matches_target_only() {
        let f = FactSourceFilter::Only(FactSource::SessionCompressed);
        assert!(f.matches(FactSource::SessionCompressed));
        assert!(!f.matches(FactSource::LLMExtracted));
    }

    #[test]
    fn excluding_skips_target_only() {
        let f = FactSourceFilter::Excluding(FactSource::SessionCompressed);
        assert!(!f.matches(FactSource::SessionCompressed));
        assert!(f.matches(FactSource::LLMExtracted));
    }

    #[test]
    fn default_is_any() {
        assert_eq!(FactSourceFilter::default(), FactSourceFilter::Any);
    }
}
```

(If `FactSource` exposes a different set of variants than `LLMExtracted`, substitute any other variant from the audit in Task 1 step 2.)

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p alephcore --lib session_search_summary::filter 2>&1 | tail -20
```

Expected: tests fail to compile if `FactSource::LLMExtracted` or whatever variant we picked doesn't exist. Adjust to whatever variants exist (re-read Task 1 step 2 audit) and re-run.

Once compiles, expected: 4 tests pass (the implementation is in the same step as the test for this small enum — it's atomic). Verify all 4 PASS.

- [ ] **Step 3: Re-export from module root**

Edit `src/memory/session_search_summary/mod.rs`. Add to the bottom:

```rust
pub use filter::FactSourceFilter;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -10`

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_search_summary/filter.rs src/memory/session_search_summary/mod.rs
git commit -m "spec-b: add FactSourceFilter enum (Any/Only/Excluding) + unit tests"
```

---

### Task 3: Pin existing note-retrieval baseline (regression guard)

**Purpose:** Spec §5.2 requires that note-retrieval (default `Any` filter) behaviour is byte-for-byte unchanged. Capture a baseline snapshot now so future tasks can assert against it.

**Files:**
- Create: `tests/spec_b_baseline_snapshot.rs`

- [ ] **Step 1: Write the snapshot capture test**

Create `tests/spec_b_baseline_snapshot.rs`:

```rust
//! Pin pre-Spec-B HybridAssembler behaviour as a regression guard.
//!
//! This test seeds a fresh in-memory store with a known fact set, runs
//! the assembler with default arguments, and asserts the returned
//! envelope matches a frozen JSON snapshot. Task 19 re-runs the same
//! seeding + assertion AFTER the FactSourceFilter parameter is added,
//! confirming default behaviour is preserved.

use alephcore::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
// Use whatever public test-helper Aleph exposes for building an in-memory
// HybridAssembler. If none, the test must construct one through the
// existing public constructors. See `src/memory/integration_tests/` for
// patterns.

#[tokio::test]
async fn note_retrieval_default_behaviour_baseline() {
    // ---- Seed ----
    // Insert the following deterministic fact set:
    //   1. note-fact A: fact_source=LLMExtracted, content "Aleph uses Rust"
    //   2. note-fact B: fact_source=UserAuthored, content "Project deadline 2026-Q3"
    //   3. session-summary fact: fact_source=SessionCompressed,
    //      path "aleph://session/xyz/d0/0", content "User asked about deployment"
    //
    // The exact MemoryFact construction calls follow the existing patterns
    // in src/memory/integration_tests/assembler_smoke.rs (cited in Task 1
    // audit if needed). Use timestamps that are deterministic (e.g. epoch
    // 1234567890).

    // ---- Run ----
    // let assembler = build_test_hybrid_assembler(...);
    // let envelope = assembler
    //     .assemble("deployment", "agent-1", None,
    //               AssemblyBudget { total_tokens: 4000 })
    //     .await
    //     .expect("assemble succeeds");

    // ---- Snapshot ----
    // Write the envelope's slot ids + ordering + render output to
    // tests/snapshots/spec_b_baseline.json. On subsequent runs assert
    // equality. Use insta crate if already a dev-dep, otherwise hand-code
    // the JSON read/write.
    todo!("Implement after locating the test-helper assembler builder; see audit step in Task 1");
}
```

The `todo!()` is **explicitly part of the discovery**. The implementer's first action in Step 2 is to locate the existing test helper and replace `todo!()` with a real implementation.

- [ ] **Step 2: Locate the existing assembler test helper**

Run:
```bash
grep -rn "HybridAssembler::new\|build_test_hybrid\|assembler_test" /Volumes/TBU4/Workspace/Aleph/src/memory --include="*.rs" | head -15
ls /Volumes/TBU4/Workspace/Aleph/src/memory/integration_tests/ 2>/dev/null
```

Expected: identify the helper or pattern. If no test helper exists, write one inline in this test file (build a minimal HybridAssembler using a fresh sqlite in-memory backend + stub reranker that returns its input verbatim).

- [ ] **Step 3: Replace `todo!()` with a real assembler invocation**

Replace the test body with concrete construction code. The exact code depends on what Step 2 finds. Below is the structural shape:

```rust
let backend = SqliteMemoryBackend::in_memory().await.expect("backend");
backend.write_fact(/* note-fact A */).await.unwrap();
backend.write_fact(/* note-fact B */).await.unwrap();
backend.write_fact(/* session-compressed fact */).await.unwrap();

let assembler = HybridAssembler::new(/* args from existing constructors */);
let envelope = assembler
    .assemble("deployment", "agent-1", None,
              AssemblyBudget { total_tokens: 4000 })
    .await
    .expect("assemble succeeds");

// Capture snapshot
let actual = envelope_to_snapshot_json(&envelope);
let expected_path = "tests/snapshots/spec_b_baseline.json";

if !std::path::Path::new(expected_path).exists() {
    // First run: write the file and pass.
    std::fs::write(expected_path, &actual).unwrap();
    eprintln!("Wrote baseline snapshot. Re-run to assert.");
    return;
}

let expected = std::fs::read_to_string(expected_path).unwrap();
assert_eq!(actual, expected, "Note-retrieval baseline drifted!");
```

Where `envelope_to_snapshot_json` is a deterministic serializer (sort by slot id; render slot kind + content excerpts; redact wall-clock fields). Implement inline as a private helper in the test file.

- [ ] **Step 4: Run the test twice**

```bash
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
```

Expected: first run writes snapshot, prints "Wrote baseline snapshot." Second run passes the equality assertion.

- [ ] **Step 5: Commit**

```bash
git add tests/spec_b_baseline_snapshot.rs tests/snapshots/spec_b_baseline.json
git commit -m "spec-b: pin pre-change note-retrieval baseline snapshot"
```

---

### Task 4: Thread `FactSourceFilter` through `WorkingMemoryAssembler::assemble`

**Files:**
- Modify: `src/memory/assembler/mod.rs` (trait signature)
- Modify: `src/memory/assembler/hybrid.rs` (HybridAssembler::assemble)
- Modify: `src/memory/assembler/gather.rs` (Gatherer query plumbing)
- Modify: every assembler call site in the codebase (default to `FactSourceFilter::Any`)

- [ ] **Step 1: Audit current call sites**

Run:
```bash
grep -rn "\.assemble(" /Volumes/TBU4/Workspace/Aleph/src --include="*.rs" | grep -v "test\|assembler/" | head -20
```

Record the list of call sites. Each gets `FactSourceFilter::Any` injected.

- [ ] **Step 2: Update the trait signature**

Edit `src/memory/assembler/mod.rs` around line 43:

```rust
use crate::memory::session_search_summary::FactSourceFilter;

#[async_trait]
pub trait WorkingMemoryAssembler: Send + Sync {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
        filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError>;
}
```

- [ ] **Step 3: Update `HybridAssembler::assemble` signature + thread the filter into `Gatherer`**

Edit `src/memory/assembler/hybrid.rs` around line 198:

```rust
#[async_trait]
impl WorkingMemoryAssembler for HybridAssembler {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
        filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError> {
        // ... existing body, but pass `filter` to the gatherer:
        let candidates = self
            .gatherer
            .gather(query, agent_id, session_id, &self.config, filter)
            .await?;
        // ... rest unchanged
    }
}
```

(Adjust the exact `self.gatherer.gather(...)` invocation to match what's currently there. The change is purely additive.)

- [ ] **Step 4: Plumb filter into `Gatherer`**

Edit `src/memory/assembler/gather.rs`. Find the `gather` method (or whichever method `HybridAssembler::assemble` calls) and add `filter: FactSourceFilter` as the last parameter. In the body, when iterating candidates, drop any whose `fact_source` doesn't match `filter.matches(...)`.

If candidate gathering goes through SQL queries that return raw rows, prefer post-filtering in Rust (cheap, correct, no SQL dialect risk) unless a `WHERE fact_source = ?` clause is trivially achievable.

```rust
// In Gatherer::gather (signature):
pub async fn gather(
    &self,
    query: &str,
    agent_id: &str,
    session_id: Option<&str>,
    config: &AssemblerConfig,
    filter: FactSourceFilter,
) -> Result<Vec<Candidate>, AlephError> {
    let raw_candidates = self.gather_raw(/* existing args */).await?;
    Ok(raw_candidates
        .into_iter()
        .filter(|c| filter.matches(c.fact.fact_source))
        .collect())
}
```

- [ ] **Step 5: Update all assembler call sites to pass `FactSourceFilter::Any`**

For each call site identified in Step 1, append `, FactSourceFilter::Any` (with the import added). Example pattern:

```rust
// Before:
let envelope = assembler.assemble(query, agent_id, sid, budget).await?;

// After:
use crate::memory::session_search_summary::FactSourceFilter;
let envelope = assembler.assemble(query, agent_id, sid, budget, FactSourceFilter::Any).await?;
```

- [ ] **Step 6: Update the baseline test from Task 3**

Edit `tests/spec_b_baseline_snapshot.rs`. Update the `assemble` call to pass `FactSourceFilter::Any`.

- [ ] **Step 7: Verify the snapshot still matches**

```bash
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
```

Expected: PASS. The default-`Any` filter produces byte-identical output to pre-change. **If this fails, the gather-side filter implementation has a bug or the pre-Spec-B candidate ordering depends on something we changed.** Stop and debug before proceeding.

- [ ] **Step 8: Add a test exercising the non-default filter**

Append to `src/memory/session_search_summary/filter.rs` (or a new `filter_integration.rs` test file):

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
    // ... use the same in-memory HybridAssembler builder as Task 3.

    #[tokio::test]
    async fn only_session_compressed_excludes_notes() {
        let assembler = build_test_hybrid_assembler_with_notes_and_session_summaries();
        let envelope = assembler
            .assemble("deployment", "agent-1", None,
                      AssemblyBudget { total_tokens: 4000 },
                      FactSourceFilter::Only(FactSource::SessionCompressed))
            .await
            .expect("assemble");
        // Assert no slot's source fact_source is anything other than
        // SessionCompressed:
        for slot in envelope.slots() {
            assert_eq!(slot.fact_source(), FactSource::SessionCompressed);
        }
        assert!(!envelope.slots().is_empty(), "session_compressed fact should appear");
    }
}
```

(Adjust property names — `slot.fact_source()`, `envelope.slots()` — to whatever the actual `MemoryEnvelope` API exposes. Audit during Step 1 of Task 3 captured this.)

- [ ] **Step 9: Run tests**

```bash
cargo test -p alephcore --lib session_search_summary 2>&1 | tail -20
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
cargo check --bin aleph-server 2>&1 | tail -10
```

Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add src/memory/assembler/ src/memory/session_search_summary/filter.rs tests/spec_b_baseline_snapshot.rs $(git diff --name-only HEAD | grep -v target)
git commit -m "spec-b: thread FactSourceFilter through WorkingMemoryAssembler"
```

---

### Task 5: Implement `lookup` — single-fact lookup by canonical path

**Files:**
- Modify: `src/memory/session_search_summary/lookup.rs`

- [ ] **Step 1: Write the failing test**

Edit `src/memory/session_search_summary/lookup.rs`:

```rust
//! Spec B — fetch the canonical /end-summary fact for a session, if it exists.
//!
//! Used by `session_search` to decide whether the lazy synthesizer must run.

use crate::error::Result;
use crate::memory::context::enums::FactSource;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::MemoryFact;

/// Returns the canonical `/end-summary` fact for `(agent_id, session_id)` if
/// one has already been written. Returns `Ok(None)` for sessions without a
/// summary yet.
pub async fn retrieve_summary_fact(
    store: &SqliteMemoryBackend,
    agent_id: &str,
    session_id: &str,
) -> Result<Option<MemoryFact>> {
    let path = format!("aleph://session/{session_id}/end-summary");
    store
        .find_fact_by_path(agent_id, &path)
        .await
        .map(|opt| opt.filter(|f| f.fact_source == FactSource::SessionCompressed))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Reuse the same in-memory backend builder as Task 3.

    #[tokio::test]
    async fn returns_none_when_no_summary_exists() {
        let store = SqliteMemoryBackend::in_memory().await.unwrap();
        let result = retrieve_summary_fact(&store, "agent-1", "sess-empty").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn returns_fact_when_summary_exists() {
        let store = SqliteMemoryBackend::in_memory().await.unwrap();
        let fact = MemoryFact::new("Test summary".to_string(), /* note_type */ Default::default(), Vec::new())
            .with_fact_source(FactSource::SessionCompressed)
            .with_path("aleph://session/sess-found/end-summary".to_string())
            .with_agent("agent-1".to_string());
        store.write_fact(&fact).await.unwrap();

        let result = retrieve_summary_fact(&store, "agent-1", "sess-found").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().content, "Test summary");
    }

    #[tokio::test]
    async fn ignores_non_session_compressed_facts_at_same_path() {
        // Defensive — if some bug ever puts a non-SessionCompressed fact
        // at /end-summary, we don't accidentally serve it as a summary.
        let store = SqliteMemoryBackend::in_memory().await.unwrap();
        let fact = MemoryFact::new("Wrong".to_string(), Default::default(), Vec::new())
            .with_fact_source(FactSource::LLMExtracted)
            .with_path("aleph://session/sess-bad/end-summary".to_string())
            .with_agent("agent-1".to_string());
        store.write_fact(&fact).await.unwrap();

        let result = retrieve_summary_fact(&store, "agent-1", "sess-bad").await.unwrap();
        assert!(result.is_none(), "non-SessionCompressed fact must not be served");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail or pass**

```bash
cargo test -p alephcore --lib session_search_summary::lookup 2>&1 | tail -20
```

Expected: tests pass IF `find_fact_by_path` already exists on `SqliteMemoryBackend`. If it doesn't, tests fail at compile time. Move to Step 3.

- [ ] **Step 3: If `find_fact_by_path` doesn't exist, add it**

Locate the existing fact-store API:

```bash
grep -n "fn find_fact\|fn get_fact\|fn fact_by_id" /Volumes/TBU4/Workspace/Aleph/src/memory/store/sqlite/*.rs | head
```

If a path-based lookup doesn't exist, add one in the appropriate sub-module of `src/memory/store/sqlite/`:

```rust
impl SqliteMemoryBackend {
    pub async fn find_fact_by_path(
        &self,
        agent_id: &str,
        path: &str,
    ) -> Result<Option<MemoryFact>> {
        // Adapt to existing query helpers — use sqlx fetch_optional or the
        // existing `query_one` pattern.
        let row = sqlx::query("SELECT ... FROM memory_facts WHERE agent = ?1 AND path = ?2 LIMIT 1")
            .bind(agent_id)
            .bind(path)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(memory_fact_from_row))
    }
}
```

(If a deserializer like `memory_fact_from_row` doesn't exist, find the existing `From<Row>` impl or follow the same pattern as `find_fact_by_id` if such exists.)

- [ ] **Step 4: Re-run tests**

```bash
cargo test -p alephcore --lib session_search_summary::lookup 2>&1 | tail -20
```

Expected: 3/3 pass.

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_search_summary/lookup.rs src/memory/store/
git commit -m "spec-b: add retrieve_summary_fact lookup by canonical path"
```

---

### Task 6: Implement `dedup` — per-session top-1 selection

**Files:**
- Modify: `src/memory/session_search_summary/dedup.rs`

- [ ] **Step 1: Write the failing test**

Edit `src/memory/session_search_summary/dedup.rs`:

```rust
//! Spec B — per-session result diversification.
//!
//! HybridAssembler returns top-K candidate facts mixed across sessions
//! (e.g. 3 d0 chunks from session A, 2 from session B, 1 from session C).
//! `top_per_session` collapses these to one best-scoring entry per
//! `session_key`, capped at `max_sessions`.

use std::collections::HashMap;

/// Minimal candidate shape used by Spec B's tool layer.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub session_key: String,
    pub agent_id: String,
    pub fact_path: String,
    pub summary_text: String,
    pub topic: Option<String>,
    pub timestamp: i64,
    pub score: f32,
}

/// Group by `session_key`, keep top score per group, take the top
/// `max_sessions` groups by group-best-score.
pub fn top_per_session(
    candidates: Vec<ScoredCandidate>,
    max_sessions: usize,
) -> Vec<ScoredCandidate> {
    let mut best_per_session: HashMap<String, ScoredCandidate> = HashMap::new();

    for c in candidates {
        let key = c.session_key.clone();
        match best_per_session.get(&key) {
            Some(prev) if prev.score >= c.score => {}
            _ => {
                best_per_session.insert(key, c);
            }
        }
    }

    let mut survivors: Vec<ScoredCandidate> = best_per_session.into_values().collect();
    // Stable order: by score descending, then session_key ascending for
    // deterministic ties.
    survivors.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.session_key.cmp(&b.session_key))
    });
    survivors.truncate(max_sessions);
    survivors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(sk: &str, score: f32, fact_path: &str) -> ScoredCandidate {
        ScoredCandidate {
            session_key: sk.to_string(),
            agent_id: "agent-1".to_string(),
            fact_path: fact_path.to_string(),
            summary_text: format!("summary-{fact_path}"),
            topic: None,
            timestamp: 0,
            score,
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = top_per_session(vec![], 5);
        assert!(out.is_empty());
    }

    #[test]
    fn single_session_multiple_chunks_collapses_to_one() {
        let input = vec![
            cand("s1", 0.5, "aleph://session/s1/d0/0"),
            cand("s1", 0.9, "aleph://session/s1/d0/1"),
            cand("s1", 0.3, "aleph://session/s1/d1/0"),
        ];
        let out = top_per_session(input, 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[0].fact_path, "aleph://session/s1/d0/1");
    }

    #[test]
    fn multiple_sessions_each_keeps_best() {
        let input = vec![
            cand("s1", 0.5, "aleph://session/s1/d0/0"),
            cand("s1", 0.9, "aleph://session/s1/d0/1"),
            cand("s2", 0.7, "aleph://session/s2/d0/0"),
            cand("s2", 0.4, "aleph://session/s2/d1/0"),
            cand("s3", 0.2, "aleph://session/s3/d0/0"),
        ];
        let out = top_per_session(input, 10);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].session_key, "s1");
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[1].session_key, "s2");
        assert_eq!(out[1].score, 0.7);
        assert_eq!(out[2].session_key, "s3");
        assert_eq!(out[2].score, 0.2);
    }

    #[test]
    fn max_sessions_caps_output_count() {
        let input = vec![
            cand("s1", 0.9, "p1"),
            cand("s2", 0.8, "p2"),
            cand("s3", 0.7, "p3"),
            cand("s4", 0.6, "p4"),
        ];
        let out = top_per_session(input, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].session_key, "s1");
        assert_eq!(out[1].session_key, "s2");
    }

    #[test]
    fn ties_broken_by_session_key_ascending() {
        let input = vec![
            cand("session-z", 0.5, "p1"),
            cand("session-a", 0.5, "p2"),
        ];
        let out = top_per_session(input, 5);
        assert_eq!(out[0].session_key, "session-a");
        assert_eq!(out[1].session_key, "session-z");
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p alephcore --lib session_search_summary::dedup 2>&1 | tail -20
```

Expected: 5/5 pass.

- [ ] **Step 3: Add a property test**

Append to the same file:

```rust
#[cfg(test)]
mod proptest {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn dedup_invariants(
            candidates in prop::collection::vec(
                (
                    "session-[a-z0-9]{1,5}",
                    0.0f32..1.0f32,
                    "aleph://session/[a-z0-9]+/d[0-2]/[0-9]+",
                ),
                0..50,
            ),
            max_sessions in 0usize..20,
        ) {
            let scored: Vec<ScoredCandidate> = candidates
                .into_iter()
                .map(|(sk, score, path)| ScoredCandidate {
                    session_key: sk,
                    agent_id: "a".into(),
                    fact_path: path,
                    summary_text: "s".into(),
                    topic: None,
                    timestamp: 0,
                    score,
                })
                .collect();
            let out = top_per_session(scored, max_sessions);

            // Invariant 1: each session_key appears at most once.
            let mut seen = std::collections::HashSet::new();
            for c in &out {
                prop_assert!(seen.insert(c.session_key.clone()),
                    "duplicate session_key {:?}", c.session_key);
            }
            // Invariant 2: result length ≤ max_sessions.
            prop_assert!(out.len() <= max_sessions);
            // Invariant 3: scores non-increasing.
            for window in out.windows(2) {
                prop_assert!(window[0].score >= window[1].score);
            }
        }
    }
}
```

- [ ] **Step 4: Run prop tests**

```bash
cargo test -p alephcore --lib session_search_summary::dedup::proptest 2>&1 | tail -20
```

Expected: PASS (proptest runs 256 cases by default).

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_search_summary/dedup.rs
git commit -m "spec-b: per-session dedup with top-1 + property tests"
```

---

### Task 7: Implement `synthesizer` — lazy on-read summary generation

**Files:**
- Modify: `src/memory/session_search_summary/synthesizer.rs`

- [ ] **Step 1: Write the skeleton + failing test**

Edit `src/memory/session_search_summary/synthesizer.rs`:

```rust
//! Spec B — lazy on-read summary synthesis for sessions without a
//! `/end-summary` fact yet.

use std::sync::Arc;

use crate::error::{AlephError, Result};
use crate::gateway::session_store::SessionStore;
use crate::memory::context::enums::FactSource;
use crate::memory::session_compactor::summary_engine::{
    build_summary_prompt, summary_to_fact, FallbackLevel,
};
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::MemoryFact;
use crate::providers::AiProvider;

/// Maximum tokens of raw transcript loaded for lazy summary synthesis.
/// See spec §10 open question 5 — provisional, tune from production data.
pub const LAZY_INPUT_MAX_TOKENS: usize = 8_000;

/// Maximum number of trailing turns considered, regardless of token count.
pub const LAZY_INPUT_MAX_TURNS: usize = 50;

#[derive(Clone)]
pub struct SummarySynthesizer {
    pub store: Arc<SqliteMemoryBackend>,
    pub session_store: Arc<dyn SessionStore>,
    pub provider: Arc<dyn AiProvider>,
}

impl SummarySynthesizer {
    /// Synthesize a `/end-summary` fact for `(agent_id, session_id)`.
    ///
    /// Behaviour:
    ///   - Loads the windowed trailing slice of the session's transcript
    ///     (capped at `LAZY_INPUT_MAX_TOKENS` / `LAZY_INPUT_MAX_TURNS`).
    ///   - Builds a leaf-depth summary prompt and runs one LLM call.
    ///   - Writes the resulting fact at canonical path with INSERT OR IGNORE.
    ///   - On a write race (a concurrent caller wrote first), reads back the
    ///     existing fact and returns that instead.
    ///   - On LLM-call failure, returns Err.
    pub async fn lazy_for(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<MemoryFact> {
        // 1. Idempotent fast-path — someone may have written between
        // session_search's lookup miss and our call.
        if let Some(existing) = super::lookup::retrieve_summary_fact(
            &self.store,
            agent_id,
            session_id,
        )
        .await?
        {
            return Ok(existing);
        }

        // 2. Load windowed transcript slice.
        let transcript = self
            .session_store
            .load_window(agent_id, session_id, LAZY_INPUT_MAX_TURNS, LAZY_INPUT_MAX_TOKENS)
            .await
            .map_err(|e| AlephError::Internal(format!("load_window failed: {e}")))?;

        if transcript.is_empty() {
            return Err(AlephError::Internal(format!(
                "no transcript for session {session_id}"
            )));
        }

        // 3. Run the summary LLM call.
        let prompt = build_summary_prompt(
            &transcript,
            /* depth */ 0,
            /* previous_context */ None,
            FallbackLevel::Normal,
        );
        let llm_output = self
            .provider
            .complete(&prompt)
            .await
            .map_err(|e| AlephError::Internal(format!("synthesizer LLM call failed: {e}")))?;
        let summary_text =
            crate::memory::session_compactor::summary_engine::strip_analysis_block(&llm_output)
                .to_string();

        // 4. Build + write fact with INSERT OR IGNORE.
        let fact = summary_to_fact(
            session_id,
            /* depth */ 0,
            /* seq */ 0,
            summary_text,
            transcript.len(),
            transcript.iter().map(|(_, c)| c.len()).sum(),
            agent_id,
        )
        .with_path(format!("aleph://session/{session_id}/end-summary"));

        self.store.write_fact_or_ignore(&fact).await?;

        // 5. Re-read in case a concurrent caller's write won the race.
        super::lookup::retrieve_summary_fact(&self.store, agent_id, session_id)
            .await?
            .ok_or_else(|| AlephError::Internal(
                "summary fact disappeared after write".to_string(),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // ... fixtures shared across synthesizer tests ...

    fn fake_provider_returning(output: &str) -> Arc<dyn AiProvider> {
        // Use an existing test-double pattern. The compactor tests already
        // construct mock providers — see `src/memory/compression/service.rs`
        // tests for the canonical `MockAiProvider` implementation.
        Arc::new(MockAiProvider::with_response(output.to_string()))
    }

    #[tokio::test]
    async fn returns_existing_fact_when_already_present() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let preexisting = MemoryFact::new("Already there".to_string(), Default::default(), vec![])
            .with_fact_source(FactSource::SessionCompressed)
            .with_path("aleph://session/preexist/end-summary".to_string())
            .with_agent("agent-1".to_string());
        store.write_fact(&preexisting).await.unwrap();

        let session_store = Arc::new(InMemorySessionStore::new()) as Arc<dyn SessionStore>;
        let synth = SummarySynthesizer {
            store: store.clone(),
            session_store,
            provider: fake_provider_returning("WOULD NOT BE USED"),
        };

        let result = synth.lazy_for("agent-1", "preexist").await.unwrap();
        assert_eq!(result.content, "Already there");
        // No LLM call was made — would be enforced by a counting mock; for now
        // the assertion above proves the short-circuit path was taken.
    }

    #[tokio::test]
    async fn synthesizes_when_no_existing_summary() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let session_store = Arc::new(InMemorySessionStore::with_messages(
            "agent-1",
            "fresh",
            &[("user", "What's deployment?"), ("assistant", "Use kubectl apply.")],
        )) as Arc<dyn SessionStore>;

        let llm_output =
            "<summary>\n## Primary Request\nDeployment question\n</summary>".to_string();
        let synth = SummarySynthesizer {
            store: store.clone(),
            session_store,
            provider: fake_provider_returning(&llm_output),
        };

        let result = synth.lazy_for("agent-1", "fresh").await.unwrap();
        assert!(result.content.contains("Deployment question"));
        assert_eq!(result.fact_source, FactSource::SessionCompressed);
        assert_eq!(result.path, "aleph://session/fresh/end-summary");

        // Second call must short-circuit (read-back path), no second LLM call.
        let again = synth.lazy_for("agent-1", "fresh").await.unwrap();
        assert_eq!(again.content, result.content);
    }

    #[tokio::test]
    async fn returns_error_when_transcript_empty() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let session_store = Arc::new(InMemorySessionStore::new()) as Arc<dyn SessionStore>;
        let synth = SummarySynthesizer {
            store,
            session_store,
            provider: fake_provider_returning("won't matter"),
        };
        let err = synth.lazy_for("agent-1", "missing").await.unwrap_err();
        assert!(format!("{err}").contains("no transcript"));
    }
}
```

- [ ] **Step 2: Locate or build `MockAiProvider` and `InMemorySessionStore` test helpers**

Run:
```bash
grep -rn "MockAiProvider\|fn complete" /Volumes/TBU4/Workspace/Aleph/src/memory/compression --include="*.rs" | head -10
grep -rn "InMemorySessionStore\|impl SessionStore for" /Volumes/TBU4/Workspace/Aleph/src --include="*.rs" | head -10
```

If neither exists publicly, write minimal helpers in `src/memory/session_search_summary/synthesizer.rs` test module:

```rust
struct MockAiProvider {
    response: tokio::sync::Mutex<Option<String>>,
    call_count: std::sync::atomic::AtomicUsize,
}
impl MockAiProvider {
    fn with_response(s: String) -> Self {
        Self {
            response: tokio::sync::Mutex::new(Some(s)),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}
#[async_trait::async_trait]
impl AiProvider for MockAiProvider {
    async fn complete(&self, _prompt: &str) -> Result<String> {
        self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .response
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "mock".into()))
    }
}
```

The `InMemorySessionStore` similarly: a minimal `HashMap<(agent, sid), Vec<(role, content)>>` with the trait impl returning `load_window` results from that map.

- [ ] **Step 3: If `SqliteMemoryBackend::write_fact_or_ignore` doesn't exist, add it**

Run:
```bash
grep -n "write_fact_or_ignore\|INSERT OR IGNORE.*memory_facts" /Volumes/TBU4/Workspace/Aleph/src/memory/store/sqlite/*.rs
```

If not present, add to the appropriate file (likely `src/memory/store/sqlite/facts.rs` or wherever `write_fact` lives):

```rust
impl SqliteMemoryBackend {
    /// Like `write_fact`, but uses INSERT OR IGNORE — caller treats a write
    /// race as success and re-reads.
    pub async fn write_fact_or_ignore(&self, fact: &MemoryFact) -> Result<()> {
        // Use the same INSERT statement as write_fact but with `OR IGNORE`.
        // Implementation mirrors the existing write_fact body — copy and
        // change the verb. See INSERT OR IGNORE patterns in
        // src/memory/store/sqlite/notes.rs:99 and recall_signals.rs:88.
        sqlx::query("INSERT OR IGNORE INTO memory_facts (id, agent, path, content, fact_source, layer, ...) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ...)")
            .bind(&fact.id)
            // ... other binds copied from existing write_fact ...
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
```

(Adjust column names + binds to whatever the existing `write_fact` uses.)

- [ ] **Step 4: If `SessionStore::load_window` doesn't exist, add it**

Run:
```bash
grep -n "load_window\|load_messages\|fn load" /Volumes/TBU4/Workspace/Aleph/src/gateway/session_store/mod.rs
```

If not present, add to the `SessionStore` trait:

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    // ... existing methods ...

    /// Load up to `max_turns` most-recent messages for `(agent_id, session_id)`,
    /// stopping early if cumulative `content.len()` exceeds `max_chars`.
    /// Returns `Vec<(role, content)>` newest-first reordered to oldest-first
    /// before return (so the prompt sees chronological order).
    async fn load_window(
        &self,
        agent_id: &str,
        session_id: &str,
        max_turns: usize,
        max_chars: usize,
    ) -> Result<Vec<(String, String)>>;
}
```

Implement in `SqliteSessionStore` and `FileSessionStore` (whichever backends exist) — for SQLite it's a `SELECT role, content FROM messages WHERE agent_id = ?1 AND session_id = ?2 ORDER BY timestamp DESC LIMIT ?3` plus reverse + char-cap.

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib session_search_summary::synthesizer 2>&1 | tail -30
```

Expected: 3/3 pass.

- [ ] **Step 6: Add concurrency test**

Append to the test module in `synthesizer.rs`:

```rust
#[tokio::test]
async fn concurrent_calls_produce_one_fact() {
    use std::sync::atomic::Ordering;

    let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
    let session_store = Arc::new(InMemorySessionStore::with_messages(
        "agent-1",
        "concurrent",
        &[("user", "Q"), ("assistant", "A")],
    )) as Arc<dyn SessionStore>;

    let provider = Arc::new(MockAiProvider::with_response(
        "<summary>\nConcurrent test summary\n</summary>".to_string(),
    ));

    // Wrap in a tiny adapter that exposes the call count.
    let synth = SummarySynthesizer {
        store: store.clone(),
        session_store,
        provider: provider.clone() as Arc<dyn AiProvider>,
    };

    let mut joinset = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let s = synth.clone();
        joinset.spawn(async move {
            s.lazy_for("agent-1", "concurrent").await.map(|f| f.content)
        });
    }

    let mut results = vec![];
    while let Some(r) = joinset.join_next().await {
        results.push(r.unwrap().unwrap());
    }

    // All ten callers see the same content.
    let first = &results[0];
    for r in &results {
        assert_eq!(r, first);
    }

    // The race window can yield 1-2 LLM calls but not 10.
    let calls = provider.call_count.load(Ordering::SeqCst);
    assert!(calls <= 2, "expected ≤ 2 LLM calls under concurrency, got {calls}");
}
```

Run:
```bash
cargo test -p alephcore --lib session_search_summary::synthesizer::tests::concurrent_calls_produce_one_fact 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/memory/session_search_summary/synthesizer.rs src/memory/store/ src/gateway/session_store/
git commit -m "spec-b: SummarySynthesizer lazy_for + transcript window loader + INSERT OR IGNORE writeback"
```

---

### Task 8: Implement `end_hook` — `SessionEndSummarizer`

**Files:**
- Modify: `src/memory/session_search_summary/end_hook.rs`

- [ ] **Step 1: Write the skeleton + failing test**

```rust
//! Spec B — `on_session_end` hook handler. Produces (or skips) a
//! `/end-summary` fact when a session truly ends.

use std::sync::Arc;

use crate::error::Result;
use crate::memory::context::enums::FactSource;
use crate::memory::session_compactor::summary_engine::summary_to_fact;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::MemoryFact;

use super::synthesizer::SummarySynthesizer;

#[derive(Clone)]
pub struct SessionEndSummarizer {
    pub store: Arc<SqliteMemoryBackend>,
    pub synthesizer: Arc<SummarySynthesizer>,
}

impl SessionEndSummarizer {
    /// Idempotent: writes a `/end-summary` fact if and only if one doesn't
    /// already exist. If `aleph://session/{sid}/d{depth}/{seq}` facts
    /// already exist (from compactor), reuses the highest-depth fact's
    /// content. Otherwise delegates to the lazy synthesizer (which does
    /// the transcript-window LLM call).
    pub async fn produce(&self, agent_id: &str, session_id: &str) -> Result<()> {
        // Step 0 — short-circuit.
        if super::lookup::retrieve_summary_fact(&self.store, agent_id, session_id)
            .await?
            .is_some()
        {
            return Ok(());
        }

        // Step 1 — try to reuse compactor d* output without an LLM call.
        if let Some(reused) = self
            .reuse_highest_depth_fact(agent_id, session_id)
            .await?
        {
            self.store.write_fact_or_ignore(&reused).await?;
            return Ok(());
        }

        // Step 2 — fall back to lazy synthesizer (one LLM call).
        let _ = self.synthesizer.lazy_for(agent_id, session_id).await?;
        Ok(())
    }

    async fn reuse_highest_depth_fact(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> Result<Option<MemoryFact>> {
        let prefix = format!("aleph://session/{session_id}/d");
        let candidates = self
            .store
            .find_facts_by_path_prefix(agent_id, &prefix)
            .await?;
        let best = candidates
            .into_iter()
            .filter(|f| f.fact_source == FactSource::SessionCompressed)
            .max_by_key(|f| extract_depth_from_path(&f.path));

        Ok(best.map(|src| {
            // Rewrite path to /end-summary and keep content + layer.
            summary_to_fact(
                session_id,
                /* depth */ 1,
                /* seq */ 0,
                src.content.clone(),
                /* source_message_count */ 0,
                /* source_token_count */ 0,
                agent_id,
            )
            .with_path(format!("aleph://session/{session_id}/end-summary"))
        }))
    }
}

fn extract_depth_from_path(path: &str) -> u32 {
    // Reuse src/memory/session_compactor/summary_source.rs::extract_depth
    // if pub. Otherwise inline the 4-line parser.
    crate::memory::session_compactor::summary_source::extract_depth(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Reuse fixtures from synthesizer.rs::tests.

    #[tokio::test]
    async fn short_circuits_when_summary_exists() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let preexisting = MemoryFact::new("already".to_string(), Default::default(), vec![])
            .with_fact_source(FactSource::SessionCompressed)
            .with_path("aleph://session/sk/end-summary".to_string())
            .with_agent("agent-1".to_string());
        store.write_fact(&preexisting).await.unwrap();

        let session_store = Arc::new(empty_session_store()) as Arc<dyn SessionStore>;
        let provider = Arc::new(MockAiProvider::with_response("X".into()));
        let synth = Arc::new(SummarySynthesizer { store: store.clone(), session_store, provider: provider.clone() });
        let hook = SessionEndSummarizer { store: store.clone(), synthesizer: synth };

        hook.produce("agent-1", "sk").await.unwrap();
        assert_eq!(provider.call_count.load(std::sync::atomic::Ordering::SeqCst), 0,
            "no LLM call expected when summary already present");
    }

    #[tokio::test]
    async fn reuses_d2_fact_without_llm() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let d0 = MemoryFact::new("d0 detail".to_string(), Default::default(), vec![])
            .with_fact_source(FactSource::SessionCompressed)
            .with_path("aleph://session/sk2/d0/0".to_string())
            .with_agent("agent-1".to_string());
        let d2 = MemoryFact::new("d2 abstract".to_string(), Default::default(), vec![])
            .with_fact_source(FactSource::SessionCompressed)
            .with_path("aleph://session/sk2/d2/0".to_string())
            .with_agent("agent-1".to_string());
        store.write_fact(&d0).await.unwrap();
        store.write_fact(&d2).await.unwrap();

        let session_store = Arc::new(empty_session_store()) as Arc<dyn SessionStore>;
        let provider = Arc::new(MockAiProvider::with_response("X".into()));
        let synth = Arc::new(SummarySynthesizer { store: store.clone(), session_store, provider: provider.clone() });
        let hook = SessionEndSummarizer { store: store.clone(), synthesizer: synth };

        hook.produce("agent-1", "sk2").await.unwrap();
        let written = super::super::lookup::retrieve_summary_fact(&store, "agent-1", "sk2")
            .await.unwrap().unwrap();
        assert_eq!(written.content, "d2 abstract", "highest depth fact reused");
        assert_eq!(provider.call_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn falls_back_to_synthesizer_when_no_d_facts() {
        let store = Arc::new(SqliteMemoryBackend::in_memory().await.unwrap());
        let session_store = Arc::new(InMemorySessionStore::with_messages(
            "agent-1", "sk3",
            &[("user", "hi"), ("assistant", "hello")],
        )) as Arc<dyn SessionStore>;
        let provider = Arc::new(MockAiProvider::with_response(
            "<summary>\nFallback summary\n</summary>".into(),
        ));
        let synth = Arc::new(SummarySynthesizer { store: store.clone(), session_store, provider: provider.clone() });
        let hook = SessionEndSummarizer { store: store.clone(), synthesizer: synth };

        hook.produce("agent-1", "sk3").await.unwrap();
        let written = super::super::lookup::retrieve_summary_fact(&store, "agent-1", "sk3")
            .await.unwrap().unwrap();
        assert!(written.content.contains("Fallback summary"));
        assert_eq!(provider.call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
```

- [ ] **Step 2: Add `find_facts_by_path_prefix` to the store if missing**

Run:
```bash
grep -n "find_facts_by_path_prefix\|path LIKE" /Volumes/TBU4/Workspace/Aleph/src/memory/store/sqlite/*.rs
```

If not present, add to the same file as `find_fact_by_path`:

```rust
pub async fn find_facts_by_path_prefix(
    &self,
    agent_id: &str,
    prefix: &str,
) -> Result<Vec<MemoryFact>> {
    let pattern = format!("{prefix}%");
    let rows = sqlx::query("SELECT ... FROM memory_facts WHERE agent = ?1 AND path LIKE ?2")
        .bind(agent_id)
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?;
    Ok(rows.into_iter().map(memory_fact_from_row).collect())
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib session_search_summary::end_hook 2>&1 | tail -30
```

Expected: 3/3 pass.

- [ ] **Step 4: Commit**

```bash
git add src/memory/session_search_summary/end_hook.rs src/memory/store/
git commit -m "spec-b: SessionEndSummarizer with d* reuse + synthesizer fallback"
```

---

### Task 9: Wire `SessionEndSummarizer` into the on-session-end hook surface

**Files:**
- Modify: `src/thinker/memory_context_provider.rs` (extend the existing `SESSION_END_MCP` slot or add a parallel `SESSION_END_SUMMARIZER` slot)
- Modify: `src/gateway/session_manager/ops.rs` (already calls `session_end_mcp()` via Spec A; add a parallel call for the summarizer)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (initialize and register the summarizer at startup)

- [ ] **Step 1: Add a parallel registration slot for the summarizer**

Edit `src/thinker/memory_context_provider.rs` near `SESSION_END_MCP`:

```rust
use crate::memory::session_search_summary::end_hook::SessionEndSummarizer;

static SESSION_END_SUMMARIZER: tokio::sync::OnceCell<Arc<SessionEndSummarizer>> =
    tokio::sync::OnceCell::const_new();

pub fn register_session_end_summarizer(summarizer: Arc<SessionEndSummarizer>) {
    let _ = SESSION_END_SUMMARIZER.set(summarizer);
}

pub fn session_end_summarizer() -> Option<Arc<SessionEndSummarizer>> {
    SESSION_END_SUMMARIZER.get().cloned()
}
```

- [ ] **Step 2: Fire the summarizer alongside the curated invalidator**

Edit `src/gateway/session_manager/ops.rs` near the existing `session_end_mcp()` call site (≈ the `emit_session_end_raw_with_registry` function used in Spec A). After the existing curated-invalidate fire-and-forget block, append:

```rust
if let Some(summarizer) =
    crate::thinker::memory_context_provider::session_end_summarizer()
{
    let agent_id = agent_id.to_string();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = summarizer.produce(&agent_id, &session_id).await {
            tracing::warn!(
                target = "spec_b.end_hook",
                agent_id = %agent_id,
                session_id = %session_id,
                error = %e,
                "session-end summarization failed (non-fatal)"
            );
        }
    });
}
```

- [ ] **Step 3: Initialize and register the summarizer at server startup**

Edit `src/bin/aleph-server/commands/start/builder/agent_init.rs`. Locate the Spec A block that calls `register_session_end_mcp(...)` (around line 1386). Append:

```rust
use alephcore::memory::session_search_summary::{
    end_hook::SessionEndSummarizer,
    synthesizer::SummarySynthesizer,
};

let synth = Arc::new(SummarySynthesizer {
    store: backend.clone(),               // Arc<SqliteMemoryBackend> already in scope
    session_store: session_store.clone(), // Arc<dyn SessionStore> already in scope
    provider: ai_provider.clone(),        // Arc<dyn AiProvider> already in scope (the same provider compactor uses)
});
let summarizer = Arc::new(SessionEndSummarizer {
    store: backend.clone(),
    synthesizer: synth.clone(),
});
alephcore::thinker::memory_context_provider::register_session_end_summarizer(
    summarizer,
);
```

(Substitute the actual variable names that hold `backend`, `session_store`, `ai_provider` in the surrounding code.)

- [ ] **Step 4: Verify compilation**

```bash
cargo check --bin aleph-server 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/thinker/memory_context_provider.rs src/gateway/session_manager/ops.rs src/bin/aleph-server/
git commit -m "spec-b: wire SessionEndSummarizer into on_session_end fire-and-forget path"
```

---

### Task 10: Update `SessionSearchHit` schema (breaking change)

**Files:**
- Modify: `src/builtin_tools/session_search.rs`

- [ ] **Step 1: Update the type definitions**

Edit `src/builtin_tools/session_search.rs`. Replace the `SessionSearchHit` and add `SummarySource`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum SummarySource {
    /// Reused from the existing session_compactor d0/d1/d2 facts.
    Compactor,
    /// Produced by the on_session_end hook backstop.
    SessionEnd,
    /// Synthesized at query time as a fallback for in-flight short sessions.
    Lazy,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub topic: Option<String>,
    /// Synthesized excerpt of the matched session (≤ 1500 chars).
    pub summary: String,
    /// 0-2 raw FTS5 snippets from the session's transcript (≤ 200 chars each).
    pub evidence_quotes: Vec<String>,
    pub timestamp: i64,
    pub source: SummarySource,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchOutput {
    pub query: String,
    pub hits: Vec<SessionSearchHit>,
    pub total_hits: usize,
}
```

The old `role` and `content` fields are removed.

- [ ] **Step 2: Update the test module to use new fields**

The existing two tests (`args_deserialization`, `args_with_max_results`) test `SessionSearchArgs` deserialization — that struct is unchanged. They continue to pass without modification.

If the file has any `SessionSearchHit { content: ..., role: ... }` construction in tests, replace with the new shape.

- [ ] **Step 3: Verify schema regeneration**

```bash
cargo check -p alephcore --lib 2>&1 | tail -10
```

Expected: clean. The next task replaces `call_impl`, so the new fields are not yet populated — that's intentional, the build only needs to compile.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/session_search.rs
git commit -m "spec-b: SessionSearchHit schema — drop content/role, add summary/evidence_quotes/source"
```

---

### Task 11: Rewrite `session_search::call_impl`

**Files:**
- Modify: `src/builtin_tools/session_search.rs`

- [ ] **Step 1: Update `SessionSearchTool` to carry the dependencies it needs**

Edit `src/builtin_tools/session_search.rs`:

```rust
use crate::memory::session_search_summary::{
    dedup::{top_per_session, ScoredCandidate},
    synthesizer::SummarySynthesizer,
    FactSourceFilter,
};
use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
use crate::memory::context::enums::FactSource;

#[derive(Clone)]
pub struct SessionSearchTool {
    context: Arc<GatewayContext>,
    caller_agent_id: String,
    assembler: Arc<dyn WorkingMemoryAssembler>,
    synthesizer: Arc<SummarySynthesizer>,
}

impl SessionSearchTool {
    pub fn new(
        context: Arc<GatewayContext>,
        caller_agent_id: impl Into<String>,
        assembler: Arc<dyn WorkingMemoryAssembler>,
        synthesizer: Arc<SummarySynthesizer>,
    ) -> Self {
        Self {
            context,
            caller_agent_id: caller_agent_id.into(),
            assembler,
            synthesizer,
        }
    }
    // ... existing is_accessible() method unchanged ...
}
```

- [ ] **Step 2: Replace `call_impl` body**

```rust
async fn call_impl(
    &self,
    args: SessionSearchArgs,
) -> std::result::Result<SessionSearchOutput, ToolError> {
    use super::{notify_tool_result, notify_tool_start};

    let args_summary = format!("搜索历史对话: {}", &args.query);
    notify_tool_start("session_search", &args_summary);

    // ① Primary retrieval — summaries only.
    let envelope = self
        .assembler
        .assemble(
            &args.query,
            &self.caller_agent_id,
            None,
            AssemblyBudget { total_tokens: 4000 },
            FactSourceFilter::Only(FactSource::SessionCompressed),
        )
        .await
        .map_err(|e| ToolError::Execution(format!("HybridAssembler failed: {e}")))?;

    // Translate envelope slots into ScoredCandidate values.
    let candidates: Vec<ScoredCandidate> = envelope
        .slots()
        .iter()
        .filter_map(|slot| {
            let fact = slot.fact()?; // adapt to actual envelope API
            let session_key = extract_session_id_from_path(&fact.path)?;
            Some(ScoredCandidate {
                session_key,
                agent_id: fact.agent.clone(),
                fact_path: fact.path.clone(),
                summary_text: fact.content.clone(),
                topic: fact.topic.clone(),
                timestamp: fact.created_at,
                score: slot.score().unwrap_or(0.0),
            })
        })
        .collect();

    // ② Per-session dedup + cap.
    let survivors = top_per_session(candidates, args.max_results);

    // ③ Build hits, fetching evidence_quotes per surviving session_key.
    let mut hits: Vec<SessionSearchHit> = Vec::new();
    for c in &survivors {
        let evidence = self
            .fetch_evidence_quotes(&args.query, &c.session_key, /* max_quotes */ 2)
            .await
            .unwrap_or_default();
        let source = source_from_path(&c.fact_path);
        hits.push(SessionSearchHit {
            session_key: c.session_key.clone(),
            agent_id: c.agent_id.clone(),
            topic: c.topic.clone(),
            summary: truncate(&c.summary_text, 1500),
            evidence_quotes: evidence,
            timestamp: c.timestamp,
            source,
        });
    }

    // ④ A2A filter (preserved from current implementation).
    hits.retain(|h| self.is_accessible(&h.agent_id));

    // ⑤ Lazy fallback for raw FTS5 hits whose session has no summary fact yet.
    // We only run this for session_keys not already covered by `hits`.
    let already_covered: std::collections::HashSet<String> =
        hits.iter().map(|h| h.session_key.clone()).collect();
    let raw_hits = self
        .context
        .session_store()
        .search_messages(&args.query, args.max_results * 4)
        .await
        .map_err(|e| ToolError::Execution(format!("session_store fallback failed: {e}")))?;

    for raw in raw_hits {
        if already_covered.contains(&raw.session_key) {
            continue;
        }
        if !self.is_accessible(&raw.agent_id) {
            continue;
        }
        if hits.len() >= args.max_results {
            break;
        }

        let synthesized = self
            .synthesizer
            .lazy_for(&raw.agent_id, &raw.session_key)
            .await;
        let (summary, source) = match synthesized {
            Ok(fact) => (truncate(&fact.content, 1500), SummarySource::Lazy),
            Err(_) => ("[summary unavailable]".to_string(), SummarySource::Lazy),
        };
        hits.push(SessionSearchHit {
            session_key: raw.session_key,
            agent_id: raw.agent_id,
            topic: raw.topic,
            summary,
            evidence_quotes: vec![raw.content], // single raw snippet as evidence
            timestamp: raw.timestamp,
            source,
        });
    }

    let total_hits = hits.len();
    let result_summary = format!("找到 {} 条历史会话摘要", total_hits);
    notify_tool_result("session_search", &result_summary, true);

    Ok(SessionSearchOutput {
        query: args.query,
        hits,
        total_hits,
    })
}

async fn fetch_evidence_quotes(
    &self,
    query: &str,
    session_key: &str,
    max_quotes: usize,
) -> std::result::Result<Vec<String>, ToolError> {
    // If SessionStore::search_messages_in_session exists, use it.
    // Otherwise post-filter from the broader search_messages.
    let raw = self
        .context
        .session_store()
        .search_messages(query, /* over-fetch */ max_quotes * 8)
        .await
        .map_err(|e| ToolError::Execution(format!("evidence search: {e}")))?;
    let mut quotes: Vec<String> = raw
        .into_iter()
        .filter(|r| r.session_key == session_key)
        .take(max_quotes)
        .map(|r| truncate(&r.content, 200))
        .collect();
    quotes.truncate(max_quotes);
    Ok(quotes)
}

fn extract_session_id_from_path(path: &str) -> Option<String> {
    // path is "aleph://session/{sid}/d0/0" OR ".../end-summary"
    path.strip_prefix("aleph://session/")?
        .split('/')
        .next()
        .map(|s| s.to_string())
}

fn source_from_path(path: &str) -> SummarySource {
    if path.ends_with("/end-summary") {
        // We can't tell SessionEnd vs Lazy at this point — both write to the
        // same path. We bias to SessionEnd as the canonical name.
        SummarySource::SessionEnd
    } else {
        SummarySource::Compactor
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}
```

- [ ] **Step 2.5: Verify the registration site has the new SessionSearchTool deps**

The tool is constructed by `BuiltinToolRegistry`. Open
`src/executor/builtin_registry/builder.rs` and find the `session_search`
registration. Update its construction to pass the new `assembler` and
`synthesizer` arguments (route via the existing OnceCell deferred-injection
pattern that Spec A used for `memory_context_provider`).

If the existing registration already has access to a `MemoryContextProvider`
(per Spec A), reuse that — both the `WorkingMemoryAssembler` and the
`SummarySynthesizer` can be exposed as accessor methods on `MemoryContextProvider`
to avoid threading more state through the registry.

```rust
// In MemoryContextProvider (Spec A code), add:
impl MemoryContextProvider {
    pub fn assembler(&self) -> Arc<dyn WorkingMemoryAssembler> {
        self.assembler.clone() // assuming Spec A already holds an Arc
    }
    pub fn summary_synthesizer(&self) -> Arc<SummarySynthesizer> {
        self.summary_synthesizer.clone() // store this in MCP at startup
    }
}

// In agent_init.rs (Task 9 already wires the synthesizer to MCP), also:
mcp.set_summary_synthesizer(synth.clone());
```

The exact wiring depends on Spec A's MCP shape — adapt as needed without
introducing a new top-level static.

- [ ] **Step 3: Verify compilation**

```bash
cargo check --bin aleph-server 2>&1 | tail -20
```

Expected: clean. Address any error mechanically.

- [ ] **Step 4: Run existing session_search tests**

```bash
cargo test -p alephcore --lib builtin_tools::session_search 2>&1 | tail -20
```

Expected: the 2 args-deserialization tests still pass (those test the request struct, which is unchanged).

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/session_search.rs src/executor/builtin_registry/ src/thinker/memory_context_provider.rs
git commit -m "spec-b: rewrite session_search.call_impl — summary-driven hits + lazy fallback"
```

---

### Task 12: Update `default_agents` system prompt

**Files:**
- Modify: `src/config/agent_resolver.rs`

- [ ] **Step 1: Locate the existing `session_search` mention**

Run:
```bash
grep -n "session_search\|search past conversations\|跨会话" /Volumes/TBU4/Workspace/Aleph/src/config/agent_resolver.rs
```

Record the section.

- [ ] **Step 2: Update the prompt text**

In the `default_agents` constant or its loaded string, replace the `session_search`-related guidance with:

```
- session_search(query, max_results=5): Search past conversations and
  retrieve summarized excerpts. Each hit is one past session, returned
  with `summary` (synthesized excerpt of what that session was about),
  `evidence_quotes` (0-2 raw transcript snippets for grounding), and
  `source` (Compactor | SessionEnd | Lazy — the most authoritative
  is Compactor when available). Use `summary` first; only consult
  `evidence_quotes` when the summary is too abstract to answer the
  question.
```

(Only the `session_search` description should change. Leave other tool descriptions untouched.)

- [ ] **Step 3: Verify the build succeeds**

```bash
cargo build --bin aleph-server 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/config/agent_resolver.rs
git commit -m "spec-b: update default_agents system prompt for new session_search schema"
```

---

### Task 13: E2E test — `fresh_short_session_lazy_synthesis`

**Files:**
- Create / modify: `tests/spec_b_e2e.rs`

- [ ] **Step 1: Write the test**

Create `tests/spec_b_e2e.rs`:

```rust
//! Spec B end-to-end integration tests.
//!
//! Each test seeds an in-memory store + session store, builds a
//! SessionSearchTool with a counting mock LLM provider, and verifies
//! the documented contract.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use alephcore::builtin_tools::session_search::{
    SessionSearchArgs, SessionSearchTool, SummarySource,
};
// Use whatever public test fixtures exist — see synthesizer.rs tests.

#[tokio::test]
async fn fresh_short_session_lazy_synthesis() {
    // Seed: one session with raw transcripts but no /d* or /end-summary fact.
    let env = TestEnv::new()
        .with_raw_session(
            "agent-1",
            "session-A",
            &[("user", "How do I deploy?"), ("assistant", "kubectl apply -f k8s/")],
        )
        .build()
        .await;
    let mock_provider = env.mock_provider();
    mock_provider.set_response(
        "<summary>\n## Primary Request\nDeployment guidance via kubectl\n</summary>".into(),
    );

    let tool = env.build_tool();
    // First call → triggers lazy synthesis.
    let result = tool.call_impl(SessionSearchArgs {
        query: "deploy".into(),
        max_results: 5,
    }).await.unwrap();
    assert!(result.hits.iter().any(|h| h.session_key == "session-A"));
    let hit = result.hits.iter().find(|h| h.session_key == "session-A").unwrap();
    assert_eq!(hit.source, SummarySource::Lazy);
    assert!(hit.summary.contains("kubectl") || hit.summary.contains("Deployment"));
    assert_eq!(mock_provider.call_count.load(Ordering::SeqCst), 1, "exactly one LLM call");

    // Second call → cache hit, NO new LLM call.
    let _ = tool.call_impl(SessionSearchArgs {
        query: "deploy".into(),
        max_results: 5,
    }).await.unwrap();
    assert_eq!(mock_provider.call_count.load(Ordering::SeqCst), 1, "no second LLM call");
}
```

Where `TestEnv` is a small builder constructed in this test file. Define it as:

```rust
struct TestEnv {
    raw_sessions: Vec<(String, String, Vec<(String, String)>)>,
    seeded_facts: Vec<MemoryFact>,
}

impl TestEnv {
    fn new() -> Self { Self { raw_sessions: vec![], seeded_facts: vec![] } }

    fn with_raw_session(mut self, agent: &str, sid: &str, msgs: &[(&str, &str)]) -> Self {
        self.raw_sessions.push((
            agent.into(),
            sid.into(),
            msgs.iter().map(|(r, c)| (r.to_string(), c.to_string())).collect(),
        ));
        self
    }

    fn with_fact(mut self, fact: MemoryFact) -> Self {
        self.seeded_facts.push(fact);
        self
    }

    async fn build(self) -> BuiltTestEnv {
        // 1. Build SqliteMemoryBackend in-memory.
        // 2. Build InMemorySessionStore seeded with raw_sessions.
        // 3. Write seeded_facts.
        // 4. Build a counting MockAiProvider.
        // 5. Build a HybridAssembler with stub reranker (passthrough).
        // 6. Build SummarySynthesizer.
        // ... return BuiltTestEnv struct holding all of these.
        unimplemented!()
    }
}

struct BuiltTestEnv { /* fields */ }

impl BuiltTestEnv {
    fn mock_provider(&self) -> Arc<MockAiProvider> { ... }
    fn build_tool(&self) -> SessionSearchTool { ... }
}
```

- [ ] **Step 2: Build the `TestEnv` helpers**

Implement the `TestEnv::build()` body using the patterns from synthesizer/end_hook tests. This is the "labour" step — copy proven construction code from existing tests, parameterize with the seeds.

- [ ] **Step 3: Run the test**

```bash
cargo test --test spec_b_e2e fresh_short_session_lazy_synthesis 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/spec_b_e2e.rs
git commit -m "spec-b/e2e: fresh short session triggers lazy synthesis + cache hit on retry"
```

---

### Task 14: E2E test — `compactor_session_uses_compressed_facts`

**Files:**
- Modify: `tests/spec_b_e2e.rs`

- [ ] **Step 1: Add the test**

Append to `tests/spec_b_e2e.rs`:

```rust
#[tokio::test]
async fn compactor_session_uses_compressed_facts() {
    let env = TestEnv::new()
        .with_fact(
            MemoryFact::new("d1 overview about Rust deployment".to_string(), Default::default(), vec![])
                .with_fact_source(FactSource::SessionCompressed)
                .with_path("aleph://session/long-sess/d1/0".to_string())
                .with_agent("agent-1".to_string())
                .with_topic("deployment"),
        )
        .build()
        .await;

    let tool = env.build_tool();
    let result = tool.call_impl(SessionSearchArgs {
        query: "deployment".into(),
        max_results: 5,
    }).await.unwrap();
    let hit = result.hits.iter().find(|h| h.session_key == "long-sess")
        .expect("long-sess hit");
    assert_eq!(hit.source, SummarySource::Compactor);
    assert!(hit.summary.contains("Rust deployment"));
    assert_eq!(env.mock_provider().call_count.load(Ordering::SeqCst), 0,
        "no LLM call expected for compactor-source hit");
}
```

- [ ] **Step 2: Run**

```bash
cargo test --test spec_b_e2e compactor_session_uses_compressed_facts 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/spec_b_e2e.rs
git commit -m "spec-b/e2e: compactor d* facts reused as summary, zero LLM calls"
```

---

### Task 15: E2E test — `session_end_hook_produces_summary`

**Files:**
- Modify: `tests/spec_b_e2e.rs`

- [ ] **Step 1: Add the test**

```rust
#[tokio::test]
async fn session_end_hook_produces_summary() {
    let env = TestEnv::new()
        .with_raw_session(
            "agent-1",
            "ended-sess",
            &[("user", "Status report?"), ("assistant", "All green.")],
        )
        .build()
        .await;
    let mock = env.mock_provider();
    mock.set_response("<summary>\n## Primary Request\nStatus report\n</summary>".into());

    // Fire the session_end hook directly (bypassing the gateway path).
    env.session_end_summarizer()
        .produce("agent-1", "ended-sess").await.unwrap();
    assert_eq!(mock.call_count.load(Ordering::SeqCst), 1);

    // Now session_search should serve the SessionEnd-source hit, no further LLM call.
    let tool = env.build_tool();
    let result = tool.call_impl(SessionSearchArgs {
        query: "status".into(),
        max_results: 5,
    }).await.unwrap();
    let hit = result.hits.iter().find(|h| h.session_key == "ended-sess").unwrap();
    assert_eq!(hit.source, SummarySource::SessionEnd);
    assert_eq!(mock.call_count.load(Ordering::SeqCst), 1, "no extra LLM call from search");
}
```

- [ ] **Step 2: Add `session_end_summarizer()` accessor to `BuiltTestEnv`**

Update the test helpers to expose the summarizer:

```rust
impl BuiltTestEnv {
    fn session_end_summarizer(&self) -> Arc<SessionEndSummarizer> {
        self.session_end_summarizer.clone()
    }
}
```

- [ ] **Step 3: Run**

```bash
cargo test --test spec_b_e2e session_end_hook_produces_summary 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/spec_b_e2e.rs
git commit -m "spec-b/e2e: session_end hook produces /end-summary fact + zero extra LLM call on search"
```

---

### Task 16: E2E test — `per_session_dedup`

**Files:**
- Modify: `tests/spec_b_e2e.rs`

- [ ] **Step 1: Add the test**

```rust
#[tokio::test]
async fn per_session_dedup() {
    // Seed five d0 facts in the same session, all matching the query.
    let mut env = TestEnv::new();
    for i in 0..5 {
        env = env.with_fact(
            MemoryFact::new(format!("Chunk {i} discusses deployment"), Default::default(), vec![])
                .with_fact_source(FactSource::SessionCompressed)
                .with_path(format!("aleph://session/big-sess/d0/{i}"))
                .with_agent("agent-1".to_string())
                .with_topic("deployment"),
        );
    }
    let env = env.build().await;

    let tool = env.build_tool();
    let result = tool.call_impl(SessionSearchArgs {
        query: "deployment".into(),
        max_results: 10,
    }).await.unwrap();

    let big_sess_count = result.hits.iter().filter(|h| h.session_key == "big-sess").count();
    assert_eq!(big_sess_count, 1, "session must appear at most once");
}
```

- [ ] **Step 2: Run**

```bash
cargo test --test spec_b_e2e per_session_dedup 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/spec_b_e2e.rs
git commit -m "spec-b/e2e: per-session dedup verified against 5-chunk same-session seed"
```

---

### Task 17: E2E test — `a2a_filter_preserved`

**Files:**
- Modify: `tests/spec_b_e2e.rs`

- [ ] **Step 1: Add the test**

```rust
#[tokio::test]
async fn a2a_filter_preserved() {
    // Seed a fact owned by agent-B; caller is agent-A which has no A2A
    // permission to reach agent-B.
    let env = TestEnv::new()
        .with_a2a_blocked("agent-A", "agent-B")
        .with_fact(
            MemoryFact::new("Sensitive content from agent-B".to_string(), Default::default(), vec![])
                .with_fact_source(FactSource::SessionCompressed)
                .with_path("aleph://session/agent-b-sess/d0/0".to_string())
                .with_agent("agent-B".to_string()),
        )
        .build()
        .await;

    let tool = env.build_tool_for_caller("agent-A");
    let result = tool.call_impl(SessionSearchArgs {
        query: "sensitive".into(),
        max_results: 5,
    }).await.unwrap();
    assert!(result.hits.iter().all(|h| h.agent_id != "agent-B"),
        "A2A filter must drop cross-agent hits");
}
```

- [ ] **Step 2: Add `with_a2a_blocked` and `build_tool_for_caller` to `TestEnv`**

Configure a stub `A2APolicy` on the `GatewayContext` that denies `(agent-A → agent-B)` access. The existing `is_accessible` code path will do the rest.

- [ ] **Step 3: Run**

```bash
cargo test --test spec_b_e2e a2a_filter_preserved 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/spec_b_e2e.rs
git commit -m "spec-b/e2e: A2A filter still applies to summary-source hits"
```

---

### Task 18: E2E test — `note_retrieval_unchanged` (snapshot regression)

**Files:**
- Modify: `tests/spec_b_baseline_snapshot.rs`

- [ ] **Step 1: Re-run the existing baseline test**

The snapshot test from Task 3 is already in the harness. After all the assembler/lookup/synthesizer/dedup/end-hook/tool changes, re-run:

```bash
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
```

Expected: PASS — note retrieval (default `Any` filter) produces output identical to the pinned snapshot.

If this fails, the most likely cause is a Gatherer change that affected ordering or content. Diagnose by inspecting the diff between the captured snapshot file and the new run output. Fix the underlying bug (do NOT update the snapshot to match — the snapshot is authoritative).

- [ ] **Step 2: Commit (only if snapshot test passes)**

No file change in this task — it's a regression check. If a fix is needed, it lands as a fix-up commit:

```bash
git commit -am "spec-b: fix gather-side filter ordering — note retrieval baseline restored"
```

---

### Task 19: Manual smoke walk-through (acceptance criterion 9)

**Files:**
- Create: `docs/superpowers/specs/2026-05-01-spec-b-smoke-log.md` (one-time evidence file, NOT a permanent doc)

- [ ] **Step 1: Build a release binary**

```bash
just build
```

Expected: clean release build of `aleph-server`.

- [ ] **Step 2: Start the server clean**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
target/release/aleph-server start
```

(Per CLAUDE.md "Process Management" warning: never run two aleph instances against the same `~/.aleph/data/`.)

- [ ] **Step 3: Drive 3 sessions with curl + the panel UI**

In separate sessions:

1. **Long session:** Carry on a 30+ turn conversation about a topic (e.g. "tell me about Rust async runtimes"). Force a context-window compaction trigger by extending until the compactor's threshold fires.
2. **Short ended session:** Have a 3-turn conversation about a different topic (e.g. "what time is it?"). Explicitly close the session via the gateway's session-close API.
3. **Short in-flight session:** Have a 3-turn conversation about a third topic (e.g. "remind me to check the Spec B log"). Leave the session OPEN.

- [ ] **Step 4: From a fourth session, run `session_search` queries**

Query each topic (`"async runtime"`, `"time"`, `"Spec B log"`). Record the responses verbatim in `docs/superpowers/specs/2026-05-01-spec-b-smoke-log.md`:

```markdown
# Spec B smoke log (2026-05-01)

## Query 1: "async runtime"
- Hit session_key: <id>
- source: Compactor
- summary: <verbatim>
- evidence_quotes: [<verbatim>, ...]

## Query 2: "time"
- Hit session_key: <id>
- source: SessionEnd
- summary: <verbatim>
...

## Query 3: "Spec B log"
- Hit session_key: <id>
- source: Lazy
- summary: <verbatim>
...
```

- [ ] **Step 5: Subjective acceptance check**

Compare against pre-Spec-B raw FTS5 output (recall the structure from `src/builtin_tools/session_search.rs` history before Task 11). Verify:

- The summary field is more useful than a raw 200-char message excerpt would have been.
- Evidence quotes ground the summary so the LLM can verify claims.
- All three sessions produced at most one hit (no duplication).

If any of these fails, file the regression and DO NOT proceed to Task 20.

- [ ] **Step 6: Commit the smoke log**

```bash
git add docs/superpowers/specs/2026-05-01-spec-b-smoke-log.md
git commit -m "spec-b: smoke walk-through evidence (acceptance criterion 9)"
```

---

### Task 20: Update roadmap + memory + reference docs

**Files:**
- Modify: `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- Modify: `docs/reference/memory/RETRIEVAL.md`
- Create: `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_b_session_search_summarization.md`
- Modify: `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md`

- [ ] **Step 1: Mark Spec B shipped in the roadmap**

Edit `docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md` "Follow-up Specs (post-roadmap)" table — change Spec B's status row to:

```
| B. session_search summarization pipeline | ✅ shipped | [design](2026-05-01-memory-evolution-spec-b-session-search-summarization-design.md) | [plan](../plans/2026-05-01-memory-evolution-spec-b-session-search-summarization.md) | 2026-05-01 |
```

- [ ] **Step 2: Append a brief subsection to RETRIEVAL.md**

Append to `docs/reference/memory/RETRIEVAL.md`:

```markdown
## Cross-session summary retrieval (Spec B)

The `session_search` tool returns one synthesized excerpt per matched
session, plus 0-2 raw evidence quotes for grounding. Summaries come from
three coordinated paths: existing compactor d0/d1/d2 facts, the
on_session_end backstop, and a lazy on-read fallback for short
in-flight sessions. All three write the same canonical fact at
`aleph://session/{sid}/end-summary` (compactor variants live at
`aleph://session/{sid}/d{depth}/{seq}`). Wiki/note retrieval
(default `FactSourceFilter::Any`) is unaffected.
```

- [ ] **Step 3: Create the memory file**

Write `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/project_spec_b_session_search_summarization.md`:

```markdown
---
name: Spec B — Session Search Summarization (SHIPPED)
description: Hermes-inspired follow-up to Spec A. session_search now returns one summarized hit per session with evidence quotes; summaries produced by compactor / on_session_end / lazy paths.
type: project
---

**Status (2026-05-01)**: ✅ SHIPPED.

**Architecture summary** (for future-session reference, not re-implementation):
- New module `src/memory/session_search_summary/` owns end_hook + synthesizer + dedup + lookup + filter.
- `WorkingMemoryAssembler::assemble` gained additive `FactSourceFilter` parameter.
- `SessionSearchHit` schema changed: dropped `content`/`role`, added `summary`/`evidence_quotes`/`source`.
- on_session_end hook from Spec 1 reused via parallel `register_session_end_summarizer` slot in `memory_context_provider.rs`.
- Lazy synthesis: 8 000 token / 50 turn windowed transcript load, `INSERT OR IGNORE` write-back, race-safe.
- Tests: 6 e2e, 1 baseline snapshot, ≥ 5 unit modules with 20+ unit tests, proptest invariants on dedup.

**How to apply (future sessions)**: Spec B is closed. If user mentions Spec C (cross-process safety beyond curated layer), that's a separate design+plan+impl cycle — start fresh with brainstorming.
```

- [ ] **Step 4: Update MEMORY.md index**

Append to `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md`:

```markdown
- [Spec B — Session Search Summarization (SHIPPED)](project_spec_b_session_search_summarization.md) — Hermes-inspired follow-up to Spec A; session_search returns summarized hits + evidence quotes.
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md docs/reference/memory/RETRIEVAL.md
git commit -m "spec-b: mark roadmap Spec B shipped + retrieval reference doc"
```

The two files in `~/.claude/...` are not in the Aleph repo — they are written via `Write` tool but not committed to Aleph's git history (per Spec A precedent).

---

### Task 21: Final acceptance review

**Purpose:** Mechanical pass against §6 of the spec. Each criterion either has a green test or a documented evidence file.

- [ ] **Step 1: Run the full Spec B test suite**

```bash
cargo test -p alephcore --lib session_search_summary 2>&1 | tail -20
cargo test --test spec_b_e2e 2>&1 | tail -20
cargo test --test spec_b_baseline_snapshot 2>&1 | tail -10
cargo clippy -p alephcore --lib --no-deps 2>&1 | tail -20
```

Expected:
- All session_search_summary unit tests pass.
- All 6 e2e tests pass.
- Baseline snapshot pass.
- 0 new clippy warnings introduced by Spec B (pre-existing warnings allowed).

- [ ] **Step 2: Cross-check each acceptance criterion**

Walk through the 9 criteria from the spec §6 and tick each:

1. Tool returns `summary` + `evidence_quotes` + `source` → covered by Tasks 13-15 e2e.
2. Long session reuses d* facts (zero LLM) → Task 14 e2e.
3. Short session paths (a) session_end hook, (b) lazy on read → Tasks 13 + 15 e2e.
4. Per-session dedup → Task 16 e2e + Task 6 proptest.
5. Existing HybridAssembler usage unaffected → Task 18 baseline snapshot.
6. A2A filter preserved → Task 17 e2e.
7. No `fact_source != SessionCompressed` ever in `session_search` → Task 4's `only_session_compressed_excludes_notes` integration test + Task 14 e2e.
8. Synthesis failure degrades gracefully → covered by Task 7 step 1 test (`returns_error_when_transcript_empty`) plus the `[summary unavailable]` branch in Task 11 step 2.
9. Manual smoke → Task 19 evidence log.

- [ ] **Step 3: Verify the dirty-files contract was honoured**

```bash
git diff HEAD~30 --name-only | grep -E "(interfaces/webchat/dist|agents/runtime|execution_engine/(engine|run_loop))" | head
```

Expected: empty output. Spec B touched none of the 5 pre-existing dirty files inherited from Spec A.

- [ ] **Step 4: Final commit (if any cleanup needed)**

If Step 1 surfaced a clippy warning introduced by Spec B, fix it now and commit:

```bash
git add .
git commit -m "spec-b: silence clippy warnings introduced by Spec B"
```

Otherwise this task closes with no extra commit.

---

## Self-review summary

(Performed after writing the 21 tasks, against the spec.)

**Spec coverage:**
- §1 motivation → no implementation work, prose only.
- §2 architecture → Tasks 1, 4, 11 establish the diagrammed components.
- §3 data model & schema → Tasks 5, 8, 10 cover storage; Task 10 covers tool schema.
- §4 data flow & lifecycle → Tasks 7 (synthesizer), 8 (end_hook), 9 (hook wiring), 11 (read path).
- §5 boundary with notes → Tasks 3 (baseline), 4 (filter additivity), 18 (snapshot regression).
- §6 acceptance criteria → covered by tests in Tasks 13-19, audited by Task 21.
- §7 test strategy → unit (Tasks 2, 5-8), e2e (Tasks 13-17), proptest (Task 6), snapshot (Tasks 3 + 18).
- §8 migration → Tasks 10 + 12 (schema break + prompt update).
- §9 YAGNI → no work; this is a non-doing list.
- §10 open questions → Task 1 audit captures the implementation-time decisions.
- §11 Spec C relationship → no work.
- §12 post-launch signals → no work in v1.

**Placeholder scan:**
- Each `todo!()` in Task 3 step 1 is explicitly replaced in step 3 of the same task.
- The "adjust property names" notes in Task 4 step 8 reference the audit findings from Task 1, which is the right place to discover them.
- Task 11 step 2 references "actual envelope API" — concrete adapter calls; the implementer is expected to read 1 file. Acceptable.

**Type consistency:**
- `FactSourceFilter` consistently named across Tasks 2, 4, 11.
- `ScoredCandidate` defined in Task 6, consumed in Task 11.
- `SummarySynthesizer` defined in Task 7, consumed in Tasks 8, 9, 11.
- `SessionEndSummarizer` defined in Task 8, consumed in Task 9.
- `SummarySource` enum consistently has 3 variants: `Compactor`, `SessionEnd`, `Lazy`.
- `retrieve_summary_fact` signature consistent across Tasks 5, 7, 8.

No gaps detected.

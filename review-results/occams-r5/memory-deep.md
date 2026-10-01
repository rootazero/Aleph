# src/memory (notes/ingest/ / scratchpad/manager.rs / integration_tests/) — occams-r5 Review

## Summary
- **7 findings**: 0 crit / 1 high / 2 med / 4 low
- Files reviewed (line counts, production+tests):
  - `scratchpad/manager.rs` — 1645L (incl. 450L tests)
  - `notes/ingest/apply.rs` — 1424L (incl. 634L tests)
  - `notes/ingest/ingestor/batch.rs` — 613L
  - `notes/ingest/ingestor/helpers.rs` — 379L
  - `notes/ingest/ingestor/plan_parse.rs` — 172L
  - `notes/ingest/retrieve.rs` — 184L
  - `notes/ingest/ref_table.rs` — 470L
  - `notes/ingest/plan.rs` — 320L
  - `notes/ingest/prompts.rs` — 240L
  - `notes/ingest/ingestor/mod.rs` — 81L
  - `notes/ingest/ingestor/tests.rs` — ~1400L
  - `notes/watcher.rs` — 184L *(covered batch-1; not re-reported)*
  - `notes/search_result.rs` — 116L *(covered batch-1; not re-reported)*
  - `integration_tests/mod.rs` — 280L
- Prior batches 1–7 covered: notes/* (batch-1), store/* (batch-3), session_compactor/session_search_summary/session_resume/session_reflection/transcript_indexer/ripple/flush (batch-5), note_retrieval/reflector/rerank (batch-6). Batch-7 touched scratchpad/manager.rs only at line ~1473 (one LOW).
- Scope: simplifying-rust-with-occams-razor. No behavior changes, no API changes.

## Findings

### [HIGH] `scratchpad/manager.rs:1495` — test loop swallows `read_dir` errors, hiding filesystem permission failures
- **Category**: quality / test correctness
- **Description**: Inside `write_roundtrips_and_leaves_no_temp_files`, the check for leftover atomic-staging files uses `while let Ok(Some(entry)) = read_dir.next_entry().await`. A permissions error on the scratchpad directory returns `Err(_)` and the loop silently exits — the test asserts `leftovers.is_empty()` and passes with 0 files examined. A real failure would not be caught.
- **Evidence**: `while let Ok(Some(entry)) = read_dir.next_entry().await { ... }` — the `Err` arm falls through the loop condition.
- **Batch-7 context**: batch-7 already flagged the same pattern in `a_note_lands_even_when_the_notes_section_is_missing` (line ~1473, also a test); this is the second occurrence in the same file.
- **Suggested fix**: `let entry = match read_dir.next_entry().await { Ok(Some(e)) => e, Ok(None) => break, Err(e) => { tracing::warn!(error = %e, "temp-file check: read_dir failed"); break; } };`.
- **Risk**: Low — test-only path. But flaky tests that silently pass are worse than red tests.

---

### [MEDIUM] `scratchpad/manager.rs` — 1645-line file exceeds 700-line threshold by 2.3x; proposed split
- **Category**: quality / maintainability
- **Description**: The file bundles six distinct concerns: (1) data types (`PlanItemStatus`, `PlanItem`, `ScratchpadSnapshot`, `PlanRenderLimits`), (2) rendering logic (`render_*`, `clamp_chars`), (3) section helpers (`section_span`, `upsert_section`, `prepend_to_section`, `find_section_start`, `normalize_single_in_progress`, `extract_section`, `parse_snapshot`), (4) `ScratchpadManager` impl, (5) public constants (`COMPLETION_BANNER`, `PROMPT_PLAN_LIMITS`), and (6) ~450 lines of tests. The test section is self-contained (`#[cfg(test)]`) and the data types are independently usable.
- **Proposed boundaries**:
  - `scratchpad/types.rs` — types + constants + render helpers
  - `scratchpad/section.rs` — pure section manipulation (`section_span`, `upsert_section`, `parse_snapshot`, etc.)
  - `scratchpad/manager.rs` — `ScratchpadManager` + tests
- **Risk**: Trivial mechanical split. `pub(crate)` visibility of section helpers must be preserved across module boundary.

---

### [MEDIUM] `notes/ingest/ingestor/helpers.rs:27–108` — `candidate_from_pageop` has 12 `#[allow(excessive-clone)]` suppressions; clone chain is a friction point
- **Category**: architecture
- **Description**: `candidate_from_pageop` clones every LLM-authored field (`title`, `category`, `tags`, `facts`, `links`) to build a `CandidateNote`. The `#[allow(excessive-clone)]` appears on every individual field assignment (12 instances). While the clones are *necessary* — `CandidateNote` owns its fields and `NoteWriteGate::evaluate` takes `&CandidateNote` — the volume signals an ownership mismatch between the `PageOp` input and the `CandidateNote` output.
- **Suggested fix**: Audit whether `CandidateNote.note` could borrow from `PageOp` (requiring a lifetime on `CandidateNote`). If not, accept the clones as the correct cost and consolidate the suppressions into a single `#[allow(clippy::excessive_clone)]` block with a comment explaining the ownership contract. The individual per-line suppressions obscure which clones are intentional vs. accidental.
- **Risk**: Low — behavior is correct. The suppressions are a maintenance friction, not a bug.

---

### [LOW] `notes/ingest/apply.rs:137–145` — `unwrap()` on static `LazyLock` regex; suppression placement matches batch-1 finding but not its recommended fix
- **Category**: quality
- **Description**: `ensure_origin_marker` uses `static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(...).unwrap())` with `// rust-doctor-disable-next-line unwrap-in-production` on the `unwrap()`. Batch-1 already flagged this exact pattern in `notes/governance/supersession.rs` and recommended `expect("...")` with a message referencing the static-pattern tests. The current suppression documents the intent but leaves the crash silent. The pattern is correct and tested, so this is a consistency note, not a regression.
- **Suggested fix**: `unwrap()` → `.expect("static origin-marker regex is verified by tests in apply.rs:tests")`. Add a `#[test]` that matches the exact pattern string.
- **Risk**: Near-zero — the pattern has been stable since the file's creation. A refactor that touches it would need to update the test anyway.

---

### [LOW] `scratchpad/manager.rs:600–700` — `set_item_status` is a 100+ line function; no structural split despite excellent inline comments
- **Category**: quality
- **Description**: `set_item_status` is 100+ lines with clear comments marking each section (single-in-progress invariant, plan-span scoping, line iteration, marker rewrite, demotion pass, bounds check). The comments are exemplary — each one documents the bug the code was fixing — but the function body is a single linear pass that could be refactored. Proposed: extract the `plan_scope_scan` (lines ~630–665) into `fn scan_plan_items(content: &str) -> Vec<ScannedItem>`.
- **Risk**: Zero — no behavior change. Refactor is optional.

---

### [LOW] `notes/ingest/ingestor/batch.rs:23` — `ingest_batch` is a ~200-line `#[allow(high_cyclomatic_complexity)]` function; structural phases are clear but the suppression is blanket
- **Category**: quality
- **Description**: `ingest_batch` chains: orientation bootstrap → related-page gather → plan → dedup → link-contract → gate → apply → orientation record → index refresh → embedding push. Each phase is clearly commented and separated by a blank line. The function is readable, but the `#[allow]` covers the entire body. Three of the phases (dedup, link-contract, gate) are themselves `#[allow(high-cyclomatic-complexity)]` — so the top-level suppression may be redundant.
- **Suggested fix**: Remove the top-level `#[allow(high_cyclomatic_complexity)]` from `ingest_batch`; keep the per-phase suppressions on `dedup_redirect_creates` (line 326) and `keyword_link_creates` (line 612). The pipeline structure is self-documenting.
- **Risk**: Low — removing the blanket suppression may trigger clippy warnings if the phases interact. Verify with `cargo clippy -p alephcore -- -W clippy::cyclomatic_complexity`.

---

### [LOW] `src/memory/integration_tests/mod.rs` — `#[cfg(test)]` module inside `src/memory/` is non-standard; project already uses `tests/memory_*.rs` workspace-level integration tests
- **Category**: architecture / test layout
- **Description**: The file is `src/memory/integration_tests/mod.rs` with `#[cfg(test)]` modules containing config sanity checks and an event-sourcing round-trip test. Rust convention places integration tests at the crate root `tests/` directory. The project already has workspace-level tests (`tests/memory_compound_ingest.rs`, `tests/memory_reflect_integration.rs`, etc.). The embedded `#[cfg(test)]` module compiles only during `cargo test` but still lives in the source tree. The deprecation note ("Graph-Augmented Retrieval integration tests have been removed") correctly documents the removal.
- **Suggested fix**: Move the event-sourcing round-trip test to `tests/memory_event_sourcing.rs` at the workspace level. The config sanity tests (`test_comptroller_config`, `test_ripple_config`) are trivial enough that they're candidates for either deletion or migration to unit tests under the relevant module.
- **Risk**: Low — moving tests may require path adjustments. Verify test coverage is preserved.

## Cross-cutting Observations

1. **`#[allow]` audit signal**: The `notes/ingest/` subtree has ~80 `rust-doctor` suppressions across production code (mostly `excessive-clone`, a few `high-cyclomatic-complexity`). The individual-per-line placement of `excessive-clone` in `helpers.rs` and `batch.rs` is the most visible friction. A consolidated suppression block per function would be more maintainable without changing behavior.

2. **Test file inflation**: `apply.rs` (790 production + 634 tests = 45% test), `ingestor/tests.rs` (~1400 lines), and `manager.rs` (1195 production + 450 tests = 27% test) together hold ~2500 lines of test code. The tests are thorough (regression-coverage for every self-healing fix documented in comments). No change needed — this is an observation, not a finding.

3. **Occam's razor satisfaction**: Despite the size, the code consistently demonstrates the stated philosophy: bug fixes are documented inline ("this used to silently X; now it Y"), `// rust-doctor-disable-next-line` is paired with a reason comment, and phase boundaries in `ingest_batch` are visually separated. No mystery dead code, no unexplained abstractions.

## Out of scope (already reviewed)
- `notes/watcher.rs` — batch-1 HIGH (debouncer unbounded channel), batch-1 MEDIUM (read_dir no metadata error), batch-7 LOW (read_dir Err dropped)
- `notes/store.rs` — batch-1 HIGH (mutex across await), batch-1 MEDIUM (note_md_filename computed on every read)
- `notes/indexer.rs` — batch-1 HIGH (relink_unresolved/prune_orphan_vectors no batch limit), batch-1 MEDIUM (full_rebuild serial fs::read_to_string)
- `notes/ingest/ingestor/mod.rs` — batch-1 MEDIUM (`expect()` on LLM-replay plan and Create gate)
- `notes/ingest/apply.rs` — batch-1 MEDIUM (unbounded regex LazyLock), now re-reported with updated fix recommendation
- `session_compactor/*`, `session_search_summary/*`, `session_resume/*`, `session_reflection/*`, `transcript_indexer/*`, `ripple/*` — batch-5
- `note_retrieval/*`, `reflector/*`, `rerank/*` — batch-6
- `store/*` — batch-3

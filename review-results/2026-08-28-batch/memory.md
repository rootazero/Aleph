# Memory Module Review (2026-08-28)

**Scope:** `src/memory/` (~12,000 lines)
**Reviewer:** static, subagent
**Files covered:** 35 top-level files (mod.rs + 17 sibling modules) + 14 subdirectories

## Summary
- **P0: 0**, **P1: 2**, **P2: 3**, (also 2 LOW / informational)
- **Total findings: 7** (5 substantive + 2 LOW)
- **Status vs prior 7-batch review (2026-08-12):** ~22 prior findings verified — **most are RESOLVED**. Significant hardening has been applied to the dreaming module, the event handler, and the store layer since the prior review.

## Status against the 7 prior batches

| Prior finding | Status | Notes |
|---|---|---|
| **batch-1** `notes/note/helpers.rs:144-170` lossy `sanitize_title` | **RESOLVED** | `sanitize_title` now rejects `..` outright (no lossy replace), per `helpers.rs:174-194` |
| **batch-1** `notes/store.rs` mutex held across awaits | **STILL PRESENT** (LOW) | The `lock_conn!` macro recovers from a poisoned mutex; `add_link_with_relation` is sync so no await — the latent risk persists in the giant `impl NoteStore` block but is not actively triggering |
| **batch-1** `notes/watcher.rs` unbounded channel | **RESOLVED** | Big-burst short-circuit now escalates to a bulk reconcile sentinel *before* allocating a per-path Vec (`watcher.rs:175-260`) |
| **batch-1** `notes/indexer.rs` `prune_orphan_vectors` no batch limit | **RESOLVED** | Now uses batched `DELETE ... WHERE rowid IN (?,?,...)` per dim table with `BATCH_SIZE = 5_000` (`store_impl.rs:1161-1218`) |
| **batch-1** `notes/governance/supersession.rs` unwrap-on-regex | **STILL PRESENT** (LOW) | Uses `LazyLock` + `.unwrap()`; unverified in this pass — pattern is short |
| **batch-2** `dreaming/mod.rs:952` no per-corpus timeout | **DEFERRED / PARTIAL** | Nightly fan-out is now budgeted by `max_corpus_cycles_per_night` and per-corpus activity-gated (`Idle`), but no wall-clock timeout per corpus |
| **batch-2** `dreaming/stages/mention_weave.rs:241,308` unsafe pointer cast | **RESOLVED** | The unsafe blocks have been replaced with safe Rust + `bytemuck::cast_slice` (or the path was rewritten — no unsafe remains in current `mention_weave.rs`) |
| **batch-2** `dreaming/stages/note_decay.rs:120-180` elapsed-seconds vs calendar | **STILL PRESENT** (LOW) | Still uses `7 * 86_400` raw seconds — the inline comment claims `chrono::Duration` but the constant is unchanged |
| **batch-2** `dreaming/event_log.rs:135` `loop {}` no end-of-stream | **RESOLVED** | `event_log.rs` was rewritten with a proper tail-reading seek/parse loop, geometric window expansion, and `read_last_measured` returning bytes_read |
| **batch-2** `dreaming/strategy.rs` `DreamStrategy::` exhaustiveness | **STILL PRESENT** (LOW) | `run_dream` uses `match` on `strategy`; new variants are caught at compile time, but production match arms in stages (e.g. `note_decay`) still `match` without explicit `_` fallthrough |
| **batch-2** `dreaming/evolution/budget.rs` `try_spend` 1-edit/1-byte | **STILL PRESENT** (LOW) | Confirmed: `try_spend(1)` returns `true` when `edits_remaining=1, bytes_remaining=1` |
| **batch-2** `dreaming/skill_gate.rs:300` sanitise bypass on paths | **RESOLVED** | `validate_skill_action` now calls `check_target_path` which rejects `..`, absolute paths, and null bytes (`skill_gate.rs:189-208`) |
| **batch-3** `store/sqlite/notes/store_impl.rs` `lock_conn!` poison recovery | **RESOLVED** | `lock_conn!` macro now logs a `tracing::warn!` on every poison event; connection cache is reset on recovery |
| **batch-3** `store/sqlite/notes/store_impl.rs:1130` `prune_orphan_vectors` DoS | **RESOLVED** | Batched `DELETE ... IN (...)` per dim table; `BATCH_SIZE = 5_000` |
| **batch-3** `store/sqlite/notes/store_impl.rs:1280` `relink_unresolved` walks all links | **RESOLVED** | Now `WHERE status = 'dangling'` with `UPDATE OR IGNORE` + dangling-dup cleanup; one prefetched resolve context (N+1 fixed) |
| **batch-3** `store/sqlite/routing_experience.rs:85-180` 3-INSERT round-trip | **STILL PRESENT** (LOW) | `record_routing_experience` still does 3 separate INSERTs (row → map → vec) with no transaction wrapping — see Finding #6 |
| **batch-3** `store/sqlite/sessions.rs` macro across-await | **STILL PRESENT** (LOW) | Latent only — current bodies are all sync; same risk as before |
| **batch-4** `assembler/hybrid.rs:303` hydrate budget | **RESOLVED** | `hydrate` now has explicit `if used >= slot.tokens_budget { break; }` and computes by characters (`hybrid.rs:447-479`) |
| **batch-4** `extensions/mcp_adapter.rs:386` panic in production | **RESOLVED** | The `panic!("expected block")` is now inside `#[tokio::test]` — no production panic remains |
| **batch-4** `events/migration.rs:234` panic on unknown variant | **RESOLVED** | The `panic!("Expected NoteMigrated")` is inside a `#[tokio::test]` block — no production panic remains |
| **batch-4** `events/handler.rs:530,750` panic in event match | **RESOLVED** | Both panics are in test code; production match arms return `Err` |
| **batch-4** `assembler/hybrid.rs:240` `clamp_pinned` underflow | **RESOLVED** | Now has `MIN_PIN_BUDGET = 4` short-circuit — pins are zeroed when budget < 4 |
| **batch-4** `assembler/render.rs` byte truncation | **STILL PRESENT** (LOW) | XML formatter still uses byte-truncated fields — not in scope for high-priority fixes |
| **batch-4** `extensions/scheduler.rs` no shutdown | **STILL PRESENT** (LOW) | Worker `loop {}` has no cancellation — same as before |
| **batch-4** `assembler/gather.rs:200-260` no shared pool cap | **STILL PRESENT** (MEDIUM) | Each leg still fetches up to `pool_limit` candidates; pool is then unioned with no per-leg cap (see Finding #2) |
| **batch-5** `session_compactor/post_turn_compress.rs:130-180` fire-and-forget spawn | **STILL PRESENT** (LOW) | Same pattern; deferred to follow-up |
| **batch-5** `session_search_summary/synthesizer.rs:561` expect on task join | **STILL PRESENT** (LOW) | Same pattern; deferred |
| **batch-6** `note_retrieval/mod.rs:200-280` `Send + Sync` bounds | **RESOLVED** | `EmbeddingProvider` trait now has explicit `Send + Sync` (`embedding_provider.rs:12`) |
| **batch-6** `compression/service.rs:401,635,699` no `JoinSet`/shutdown | **STILL PRESENT** (MEDIUM) | 3 spawn sites still fire-and-forget — see Finding #7 |
| **batch-6** `curated/store.rs:24,131` `Mutex<()>` I/O gate | **STILL PRESENT** (LOW) | Still uses `tokio::sync::Mutex<()>`; writes are serialised (see Finding #5) |
| **batch-6** `context_comptroller/comptroller.rs` `u32` token budget | **RESOLVED** | `ComptrollerConfig::token_budget` is now `usize` with `default = 100_000` (`config.rs:13`) |
| **batch-6** `note_retrieval/scoring.rs` MMR lambda hardcoded | **RESOLVED** | `mmr_lambda` is exposed on `RetrievalScoringConfig` and consumed by the pipeline |
| **batch-7** `project_scope.rs:155-200` `list_note_corpora` silently flattens | **RESOLVED** | Uses `let mut ids = Vec::new(); for entry in entries { ... }` with per-entry `warn!` on `Err` |
| **batch-7** `insights.rs:120-150` silent truncation | **RESOLVED** | `ToolUsageReport.truncated: bool` is now exposed to admin RPC |
| **batch-7** `streaming_scrubber.rs` no iteration cap | **STILL PRESENT** (LOW) | `loop {}` has no per-call `MAX_TOKENS` |
| **batch-7** `reembed.rs:1-100` no rate limit | **STILL PRESENT** (LOW) | No `token_bucket` or rate-limit knob |
| **batch-7** `embedding_manager.rs:22-36` `Arc<RwLock<...>>` across await | **RESOLVED** | Uses `tokio::sync::RwLock`; `flush_pending` separates drain/lock from embed call |
| **batch-7** `content_scanner.rs:10,200` unanchored regex | **RESOLVED** | All patterns are anchored with `\b` and `(?i)`; unwrap replaced with `.expect("...regex must compile")` |
| **batch-7** `embedding_resolver.rs` unknown model panic | **STILL PRESENT** (LOW) | Default arm returns `AlephError::config` — confirmed via grep, actually resolved; ignore this row |

**Net:** The dreaming module (which was the review focus area) has been **substantially hardened** since the 2026-08-12 review: best-health checkpoint is persisted, the cycle-timeout-then-walrus-slack shape is solid, per-namespace sub-cycles land in their own event logs, the event-log reader is bounded, the feedback-distill Goodhart counter-metric is fixed, the recall-signals retention cleanup is wired into the dream pipeline, and the idle-sensor is reconnected. The prior `note_decay.rs` calendar-days "fix" and `try_spend` 1-edit/1-byte bug were not picked up — the comments claim a fix that the code does not actually apply.

---

## Findings

### [P1] `dreaming/stages/note_decay.rs:178` — `7 * 86_400` raw seconds protection window contradicts the file's own doc-comment
- **Category:** logic
- **Confidence:** High
- **Prior finding:** batch-2 `note_decay.rs:120-180` (claimed RESOLVED in inline comment, but the code is unchanged)
- **Description:** The doc-comment says: "Calendar days via chrono::Duration, not 7*86400 raw seconds: 7*86400 is exactly 7.0 solar days and ignores leap seconds; a 1-hour NTP correction at the boundary can flip a borderline note in or out of protection." The implementation immediately below uses exactly that:
  ```rust
  let age_seconds = now - note.created_at;
  const SEVEN_DAYS_SECS: i64 = 7 * 86_400;
  if age_seconds < SEVEN_DAYS_SECS {
      notes_protected += 1;
      continue;
  }
  ```
  This is a comment-vs-code mismatch: the reader is told the constant is wrong, but the constant ships anyway. The same `<` (strictly less than) also means a note created at the boundary slips through.
- **Impact:** Notes whose `created_at` is within 1 second of 7-days-old are silently archived. NTP corrections across the boundary corrupt the deterministic protection window.
- **Suggested fix:** Either (a) drop the misleading comment and accept `7*86400` as deliberate; or (b) compute the cutoff as `now - chrono::Duration::days(7).num_seconds()` and switch to `>=`. Pick one; today both branches are documented.

### [P1] `dreaming/evolution/budget.rs:53-65` — `EditBudget::try_spend` accepts a 1-byte edit even when the budget is exhausted-by-bytes
- **Category:** logic
- **Confidence:** High
- **Prior finding:** batch-2 `evolution/budget.rs` (carried over, exact code path described in batch-2)
- **Description:** The condition is `if self.edits_remaining == 0 || self.bytes_remaining < bytes { return false; }`. With `edits_remaining = 1, bytes_remaining = 1`, `try_spend(1)` returns `true` and decrements both counters to 0 — a "1 byte is too small for a meaningful supersede" edit still slips through and is treated as a real destructive spend.
- **Impact:** A near-exhausted budget can fund one more destructive edit (write a 1-byte supersede note, immediately re-distilled by next cycle). The clamp-by-min-bytes suggestion (MIN_SUPERSEDE_BYTES) was never added; the budget silently admits degenerate edits.
- **Suggested fix:** Either (a) `if self.edits_remaining == 0 || self.bytes_remaining < bytes.max(MIN_EDIT_BYTES)`, or (b) split `try_spend` into separate edit-count and byte-count probes so the caller must check both.

### [P2] `assembler/gather.rs:49-89` — pool is the union of N legs each capped at `pool_limit`, so the post-merge cap is `N × pool_limit`
- **Category:** performance
- **Confidence:** High
- **Prior finding:** batch-4 `gather.rs:200-260` (latent; partially mitigated by per-leg content diversity, but the unbounded-by-N claim still holds)
- **Description:** Six legs (`notes`, `snapshot`, `raws`, `profile`, `feedback_floor`, `daily_insight`) each call out with `input.pool_limit` as their bound. The merged `pool` then has up to `6 × pool_limit` entries before the post-gather `FactSourceFilter::matches` retain. With `pool_limit = 100` (the high end of the configured range), a busy agent could see 600 candidates pre-filter.
- **Impact:** A misconfigured `pool_limit` becomes `N × pool_limit` candidates for the downstream LLM rerank to score — wasted tokens and latency, and the LLM's rerank pass degrades (more candidates = noisier ranking). Same shape as the assembler/gather.rs:485 and :568 test-only panics; those tests are in `#[cfg(test)]`, fine.
- **Suggested fix:** Cap each leg to `pool_limit / N_LEGS` (here 6, so `pool_limit / 6` rounded up) so the worst case respects `pool_limit`. The doc-comment already promises this; the code does not deliver it.

### [P2] `assembler/gather.rs:254` — `daily_insight` fetch swallows `Err(_)` then returns an empty Vec silently inside `fetch_daily_insight`
- **Category:** error-handling
- **Confidence:** Medium
- **Prior finding:** new
- **Description:** `fetch_daily_insight` (lines ~245-290) iterates `[today, yesterday]`; on the first `Err(e)` from `get_daily_insight`, it `warn!`s and returns `Vec::new()` — but never tries the second date. A transient backend error on today's date silently drops yesterday's insight too.
- **Impact:** The dream daemon's `DailyDigestStage` writes every night, but a single transient SQLite hiccup on the read path suppresses the digest from two days of context. The aggregator's caller has no way to distinguish "no digest yet" from "backend is broken".
- **Suggested fix:** `continue` on `Err(e)` so the next-date fallback fires; only return `Vec::new()` after both dates are exhausted.

### [P2] `curated/store.rs:24, 131, 368` — `tokio::sync::Mutex<()>` serialises every concurrent write; a slow disk blocks all readers
- **Category:** performance / concurrency
- **Confidence:** High
- **Prior finding:** batch-6 `curated/store.rs:24,131`
- **Description:** `io_gate: tokio::sync::Mutex<()>` and `let _gate = self.io_gate.lock().await;` serialise every `with_lock` call. The prior review's suggested `Semaphore::new(MAX_CONCURRENT_WRITES)` was not applied. With one agent's curated store, concurrent tool calls (e.g. `remember` + `forget` from a parallel session) block each other; on a slow disk (NFS, USB SSD) the lock is held for hundreds of ms.
- **Impact:** Under concurrent writes the curated store becomes a serialisation point. The tool layer presents `add` / `replace` / `remove` / `batch` as independent operations; they are not.
- **Suggested fix:** `Semaphore::new(MAX_CONCURRENT_WRITES)` (suggest 4); critical section is the disk I/O, not the in-memory `state`.

### [LOW] `compression/service.rs:401, 699` — `tokio::spawn` for turn-threshold compression has no `JoinSet`, no shutdown
- **Category:** architecture
- **Confidence:** High
- **Prior finding:** batch-6 `compression/service.rs:401,635,699`
- **Description:** Two `tokio::spawn` sites fire-and-forget. `compression.cancel` admin RPC has no abort path. Long-running sessions can leak background tasks on daemon shutdown.
- **Suggested fix:** Wrap in `JoinSet`; have `cancel` abort the set.

### [LOW] `store/sqlite/routing_experience.rs:90-148` — `record_routing_experience` does 3 sequential INSERTs without a transaction; panic between steps 1 and 3 leaves an orphan vec_map row
- **Category:** logic
- **Confidence:** High
- **Prior finding:** batch-3 (carried over; no fix applied)
- **Description:** The row insert, the map insert, and the vec insert are 3 separate `conn.execute` calls. A panic or process kill between steps 1 and 3 leaves a `routing_exp_vec_map` row that `recall_routing_experience` will read but never resolve.
- **Suggested fix:** Wrap in `conn.transaction()`; panic-rollback propagates from the dropped `Transaction`. Add `prune_orphan_routing_experiences` analogous to the notes version.

---

## Cross-cutting observations

1. **Dreaming module is now structurally solid.** The `best_health` checkpoint persists, the per-namespace sub-cycle owns its own event log, the activity probe is properly threaded into both `check_and_run` (entry precondition) and `DreamPipeline::run` (per-stage yield). The prior review's structural concerns about "vacuous interruptions" and "audit rows missing" are addressed end-to-end (single writer `persist_run_row` + vacuous-night skip gate + `is_vacuous_interruption` predicate). The lingering issues are localised arithmetic / control-flow details, not architecture.

2. **The event handler has been rewritten to return errors instead of panicking.** Every `panic!` remaining in `events/` is inside a `#[test]` block. This is the largest single behavioural improvement since the prior review.

3. **`unreachable!` and `panic!` are still present in tests** (gather.rs:485, 568; curated/store.rs:678, 763; compression/scheduler.rs:111, 126; notes/*/tests.rs; etc.). These are all inside `#[cfg(test)]` / `#[test]` / `#[tokio::test]` — fine.

4. **The store layer's batched SQL improvements are consistent.** `prune_orphan_vectors` (batched `IN`), `relink_unresolved` (indexed by `status`), `routing_experience` (still 3 un-transacted INSERTs — Finding #6) — most are wired through `conn.unchecked_transaction()` or batched deletes. `routing_experience` is the outlier.

5. **`unwraps` in the dreams layer are concentrated in test fixtures**, with a few exceptions in production paths (`from_hms_opt(...).unwrap()` in `mod.rs:2138-2149` — uses `chrono::NaiveTime::from_hms_opt` which is `Option`, but with literal hour/min/sec, the `unwrap` is safe by construction). Same pattern in `note_decay.rs` and `mention_weave.rs`.

6. **`tokio::sync::RwLock` is now the norm** for `Async` paths (EmbeddingManager, etc.) and `std::sync::Mutex` for pure-sync paths. The cross-await lock risk is no longer acute.

7. **Curated store I/O gate** is the one place where the architectural risk surface (Mutex held across async disk I/O) was *not* upgraded to `Semaphore` despite the prior review's clear recommendation.

8. **The prior `try_spend` and `note_decay` "fixed" comments** describe fixes that did not actually land in the code. This is the most concrete regression-risk finding from this pass — a comment claiming the constant is correct when the code below uses it as written is worse than no comment at all, because a future refactor will trust the comment and not re-verify the constant.

## Architecture compliance

- **R1 (Core never calls platform APIs):** ✅ Compliant. `note_path_resolver` and the `MemoryStore` trait abstracts the platform-specific I/O.
- **R3 (Core minimalism):** ✅ Mostly compliant. The note layer has heavy LLM-driven pipelines (skill_distill, feedback_distill, tool_failure_distill) but they are gated behind `EditBudget` and proper watermark / cooldown machinery.
- **R4 (Interface layers are pure I/O):** ✅ Compliant. `MemoryCommandHandler` is a write-side facade with no business logic of its own — it dispatches to `EventProjector::fold_events_to_note`.
- **R7 (One core, many shells):** ✅ Compliant. The store trait is the single seam; SQLite is one impl, no business logic in the platform code.
- **R10 (Intelligence lives in the prompt):** ⚠️ Partial. Several `dreaming/stages/*` files embed magic numbers (e.g. `0.95` similarity thresholds, `MERGE_ACCEPT_THRESHOLD`) that are policy-like but not configurable. The skill_gate threshold is hardcoded at 0.5, the merge threshold at 0.6, the decay `half_life_days = 90.0` defaults. Most of these have config wiring in `config::types::memory::*` but the stages sometimes fall back to constants (e.g. `note_decay.rs:84` defaults).

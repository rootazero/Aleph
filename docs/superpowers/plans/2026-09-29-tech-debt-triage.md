# Tech Debt Triage Plan (Tier A + B)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut dead `update_session_usage` end-to-end, fix the stale `manual.rs` test invariant to match current safety property, reorder metadata-lock acquisition in four `file_backend` operations, and audit + fix the zero-token-billed-as-paid path.

**Architecture:** No new subsystems. Four surgical fixes inside existing modules: `session_manager/ops`, `context/compact`, `session_store/file_backend`, `providers/metering` × `spend`. Each task is independently testable; together they close the user-flagged 搁置 line except `snap_out_of_open_run` (deliberately untouched — safe over-protect) and "replay range read" (deferred — no perf evidence).

**Tech Stack:** Existing Rust + Tokio + existing `MetaLocks` (file_backend) + existing `SpendLedger` trait. No new dependencies.

**Spec:** `docs/superpowers/specs/` (none — bounded cleanup). Plan carries the design.

---

## Global Constraints

- **MSRV 1.95** (Cargo.toml); CI pinned to 1.96.0; no toolchain changes.
- **No new `pub` surface** — every CUT removes more than it adds.
- **Sync primitives**: import from `crate::sync_primitives`, never `std::sync` directly. The `MetaLocks` API in `session_store/file_backend/meta.rs` already obeys this.
- **Lock hierarchy** (CLAUDE.md E.0): meta lock → transcript writes → caller event. Sequential, never nested.
- **Tests**: keep coverage; CUTs delete tests for the CUT method, retain tests for `stamp_and_bill_in_range`.
- **Verification set per CLAUDE.md**: `cargo check -p alephcore`, `cargo test -p alephcore --lib --no-run`, `cargo test -p alephcore --bins`, `cargo clippy --workspace --all-targets` (with placeholder pre-step).
- **No feature flags** introduced; CUTs stay production-clean.

## Review Focus

| # | Input class / failure mode | Expected behavior | Owning task |
|---|----------------------------|-------------------|-------------|
| R1 | Two concurrent `truncate_messages` + `restore_checkpoint` on same session | Both observe consistent `(transcript, message_count)` pairs; no torn state | Task 3 |
| R2 | A test stub or trait default impl for `update_session_usage` is missed by the grep | None — every site in the cut-set is verified gone | Task 1 |
| R3 | A future caller reintroduces `update_session_usage` to bill a run | Already blocked: `stamp_and_bill_in_range` is the only billing entry; `notify_usage_updated` exists for cross-trait emission | Task 1 (doc) |
| R4 | `manual.rs` invariant rewritten looser than current behavior allows | Invariant must still cover the "same id two segments" case (covered by the existing event_snap test) | Task 2 |
| R5 | Zero-token + 0-cost call still writes a row visible to `principals_in` | Skipped entirely — row never created | Task 4 |

---

## File Structure

| File | Responsibility (this plan) |
|------|-----------------------------|
| `src/gateway/session_store/mod.rs` | Drop `update_session_usage` from `SessionStore` trait |
| `src/gateway/session_store/file_backend/mod.rs` | Drop method impl + delete 5 sites of usage (4 functions + 1 test) |
| `src/gateway/session_store/sqlite_backend/mod.rs` | Drop method impl |
| `src/gateway/session_manager/ops/modify.rs` | Drop `update_session_usage` definition + module-doc line + sibling ref |
| `src/gateway/session_manager/ops/crud.rs` | Drop the explanatory comment line |
| `src/gateway/session_manager/tests.rs` | Drop 5 test calls; keep `stamp_and_bill_in_range` coverage |
| `src/gateway/handlers/projects_channel.rs` | Drop 2 test-mock lines |
| `src/memory/session_search_summary/{end_hook.rs,synthesizer.rs}` | Drop 1 test-mock line each |
| `tests/spec_b_e2e.rs` | Drop 1 test-mock line |
| `src/context/compact/manual.rs` | Rewrite `assert_invariant_run_markers_balanced` to express "kept-side marker balance"; add one regression for same-id-double-segment |
| `src/context/compact/manual.rs` (loom-friendly file already in module) | Loom test: concurrent truncate + restore on same key |
| `src/providers/metering.rs` | Skip record path when token total + delta are both zero |

No new files. Loom test goes in the existing `loom_concurrency.rs` companion file under `session_store/file_backend/` if it exists, else appended to the file backend's existing test module.

---

## Task 1: CUT `update_session_usage`

**Files:**
- Modify: `src/gateway/session_store/mod.rs:557-580` (trait method + doc)
- Modify: `src/gateway/session_store/file_backend/mod.rs:1474` (impl)
- Modify: `src/gateway/session_store/sqlite_backend/mod.rs:532` (impl)
- Modify: `src/gateway/session_manager/ops/modify.rs:10, 540-578` (module-doc + fn)
- Modify: `src/gateway/session_manager/ops/crud.rs:355-358` (comment)
- Modify: `src/gateway/session_manager/tests.rs:1115-1175` (test calls — 5 sites)
- Modify: `src/gateway/handlers/projects_channel.rs:1105-1119` (test mock — 2 sites)
- Modify: `src/memory/session_search_summary/end_hook.rs:300-303` (test mock)
- Modify: `src/memory/session_search_summary/synthesizer.rs:348-351` (test mock)
- Modify: `tests/spec_b_e2e.rs:349-352` (test mock)

**Interfaces:**
- Consumes: existing `notify_usage_updated(&str)` (no change)
- Removes: `SessionStore::update_session_usage` from the trait surface

- [ ] **Step 1: Read every grep hit (already done)** — confirm no prod caller outside `modify.rs`. `crud.rs:357` comment confirms.

- [ ] **Step 2: Drop trait method** in `session_store/mod.rs:557-580` (delete method + its preceding doc-comment lines).

- [ ] **Step 3: Drop impl in `file_backend/mod.rs:1474`** — delete the method body only, leave the rest of `impl SessionStore` intact.

- [ ] **Step 4: Drop impl in `sqlite_backend/mod.rs:532`** — same.

- [ ] **Step 5: Drop `SessionManager::update_session_usage` in `ops/modify.rs:540-578`** + adjust module-doc line at `:10`.

- [ ] **Step 6: Drop the now-stale comment in `ops/crud.rs:355-358`** (the parenthetical).

- [ ] **Step 7: Drop test calls + mocks** at the 5 test-file sites (session_manager/tests.rs, projects_channel.rs, session_search_summary/{end_hook,synthesizer}.rs, spec_b_e2e.rs).

- [ ] **Step 8: Run `cargo test -p alephcore --bins --no-run`** — Expected: PASS, no `unused method` warning.

- [ ] **Step 9: Run `cargo check -p alephcore --all-targets`** — Expected: PASS, no `dead_code` warning.

- [ ] **Step 10: Run `cargo clippy -p alephcore --all-targets -- -D warnings`** — Expected: PASS.

- [ ] **Step 11: Commit** `gateway: cut update_session_usage (dead code, no prod caller)`

---

## Task 2: Fix `manual.rs` test invariant

**Files:**
- Modify: `src/context/compact/manual.rs:820-849` (`assert_invariant_run_markers_balanced`)
- Modify: `src/context/compact/manual.rs` (add new regression test next to `assert_invariant_run_markers_balanced`)

**Interfaces:**
- Consumes: existing `snap_out_of_open_run` (no change)
- Produces: an invariant function that asserts the actual safety property

- [ ] **Step 1: Rewrite the invariant doc-comment** to state: "every `RunStarted` whose `RunFinished` is in the kept tail must itself be in the kept tail." (Current wording assumes unique ids — wrong; the snap function already pairs each closer with the LAST opener.)

- [ ] **Step 2: Adjust the loop body** to use the same pattern `event_snap::snap_out_of_open_run` uses for "last opener under this id": `events[..cut].iter().rposition(...)` instead of any-existence assumption.

- [ ] **Step 3: Add a new test** `a_reused_run_id_keeps_both_markers_balanced` exercising same-id-double-segment at the `manual.rs` level (not just `event_snap.rs`) so the invariant is pinned at its own assertion site.

- [ ] **Step 4: Run `cargo test -p alephcore --lib context::compact::manual`** — Expected: PASS.

- [ ] **Step 5: Run `cargo test -p alephcore --lib context::compact::event_snap`** — Expected: PASS (sanity).

- [ ] **Step 6: Commit** `compact: rewrite manual.rs run-marker invariant to match current snap semantics`

---

## Task 3: Reorder metadata-lock acquisition in `file_backend`

**Files:**
- Modify: `src/gateway/session_store/file_backend/mod.rs:756-770` (`reset_session`)
- Modify: `src/gateway/session_store/file_backend/mod.rs:916-957` (`truncate_messages`)
- Modify: `src/gateway/session_store/file_backend/mod.rs:959-998` (`delete_messages_from_seq`)
- Modify: `src/gateway/session_store/file_backend/mod.rs:1089-1124` (`restore_checkpoint`)

**Interfaces:**
- Consumes: `self.lock_metadata(&key_str)` → `MetaGuard`
- Produces: 4 functions whose transcript write happens UNDER the metadata lock

- [ ] **Step 1: Read `meta::MetaLocks::lock` doc** — confirm the guard scope covers the whole `commit()` lifetime; if not, this plan needs restructuring.

- [ ] **Step 2: Write loom test** `concurrent_truncate_and_restore_on_same_session_observes_consistent_meta` in `src/gateway/session_store/file_backend/mod.rs`'s test module — two threads, one truncates from N to N-K, one restores to checkpoint M, both observe consistent `(transcript_len, message_count)`.

- [ ] **Step 3: Run the loom test on the CURRENT code** — Expected: FAIL or hang, demonstrating the race is detectable.

- [ ] **Step 4: Fix `truncate_messages`** — move `lock_metadata(&key_str).await?` to BEFORE the `atomic_write_file(&path, ...)` call; keep the read above the lock too, OR keep the read above (it doesn't touch disk state), and put only the write + meta update under the lock. Standard pattern: `lock → read transcript → write new transcript → update meta.message_count → commit`. Drop the lock before returning.

- [ ] **Step 5: Fix `delete_messages_from_seq`** — same pattern.

- [ ] **Step 6: Fix `restore_checkpoint`** — same pattern. The current code already takes the lock for the meta update; move it up before the write.

- [ ] **Step 7: Fix `reset_session`** — same pattern. (User's list didn't mention this; per CLAUDE.md 判据 #16 fix the twin.)

- [ ] **Step 8: Re-run the loom test** — Expected: PASS (or at least not racy).

- [ ] **Step 9: Run `cargo test -p alephcore --lib gateway::session_store::file_backend`** — Expected: PASS, no regressions.

- [ ] **Step 10: Run `cargo test -p alephcore --lib --no-run`** — Expected: PASS.

- [ ] **Step 11: Run `cargo clippy -p alephcore --all-targets -- -D warnings`** — Expected: PASS.

- [ ] **Step 12: Commit** `session_store/file_backend: lock metadata before transcript rewrite (4 functions)`

---

## Task 4: Audit + fix zero-token-billed-as-paid

**Files:**
- Investigate: `src/providers/metering.rs:82-256` (record_usage → record_spend → record_spend_with → ledger.record)
- Investigate: `src/spend/mod.rs:233-280` (`record` impl)
- Investigate: `src/orchestrator/dispatch.rs` (TokenBreakdown::from)
- Modify: `src/providers/metering.rs` (skip-when-zero branch) OR `src/spend/mod.rs` (skip-when-zero inside record)

**Interfaces:**
- Consumes: existing `record_spend_with`
- Produces: a behavior where a zero-token + zero-cost call does not create a row

- [ ] **Step 1: Audit: trace what happens when `usage.input_tokens + usage.output_tokens == 0` AND `pricing_result.usd == 0.0` AND `CostStatus::Complete`.** — Trace through `record_spend_with`: it computes `estimate = estimate(...)`. If `pricing` returns `Complete { usd: 0.0 }`, the code constructs `Delta::Usd(0.0)`. That hits `spend::InMemorySpendLedger::record` with `Delta::Usd(0.0)`: `row.usd += 0.0` (no-op), but `is_new_row` branch creates the row AND adds to `by_period` index. The call shows up in `principals_in`. Confirmed bug surface.

- [ ] **Step 2: Audit the SQLite backend** — same `record` semantics? Check `src/spend/sqlite.rs:112`.

- [ ] **Step 3: Decide the fix surface.** Two options:
  - (a) At the call site `record_spend_with`: if `total_tokens == 0` AND `delta == Delta::Usd(0.0)` (or `Partial(0.0)`), early-return without calling `ledger.record`.
  - (b) Inside `SpendLedger::record`: detect the no-op case and skip.
  — Prefer (a): the ledger is "record what happened"; the metering layer is the right place to say "nothing billable happened."

- [ ] **Step 4: Implement (a) in `record_spend_with`** — after computing `delta`, check `if delta.usd() == Some(0.0) && usage.input_tokens + usage.output_tokens == 0`; early-return. (`Delta::Unpriced` should still record — that's the explicit "we don't know the price" signal.)

- [ ] **Step 5: Write test** `zero_token_zero_dollar_call_skips_record` in `providers/metering.rs` test module. Add complementary `nonzero_token_unpriced_still_records` to lock in the "Unpriced still records" path.

- [ ] **Step 6: Run `cargo test -p alephcore --lib providers::metering`** — Expected: PASS.

- [ ] **Step 7: Run `cargo test -p alephcore --lib spend`** — Expected: PASS, no regression in the existing ledger tests.

- [ ] **Step 8: Commit** `metering: skip ledger record when token total + USD are both zero`

---

## Self-Review

After all four tasks complete:

1. **Spec coverage** (vs. user-flagged 搁置 list):
   - Item 1 (snap_out_of_open_run edge case) — **deliberately not in this plan** (safe over-protect, see triage).
   - Item 2 (manual.rs invariant) — Task 2 ✓
   - Item 3 (metadata lock) — Task 3 ✓
   - Item 4 (update_session_usage) — Task 1 ✓
   - Item 5 (zero token + 0 dollar) — Task 4 ✓
   - Item 6 (replay range read) — **deliberately not in this plan** (no perf evidence).
   - Phase 3 deferred items — out of scope; user said "也启动" → brainstorming in chat, not in this plan.
   - Negative space — out of scope; merge checklist.

2. **Step scan** — every step has either (i) a specific file:line + change, (ii) a test name + command, or (iii) a verification command. No "handle error cases" or "TBD."

3. **Type consistency** — `notify_usage_updated(&str)` is preserved (it serves `stamp_and_bill_in_range` already); no other signature changed in Task 1. Task 3 keeps `MetaGuard` API intact. Task 4 keeps `Delta` enum intact.

4. **Review Focus** — R1 covered by Task 3 loom test; R2/R3 covered by Task 1 grep + commit; R4 covered by Task 2 regression test; R5 covered by Task 4 unit test.

5. **Proportion** — 4 tasks, each ≤12 steps. No code blocks included (per "transcript vs plan" rule). Plan length ~ ok.
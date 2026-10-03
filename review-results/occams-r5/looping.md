# src/looping — occams-r5 Review

## Summary
- 4 findings: 0 crit / 0 high / 0 med / 4 low

## Findings

### [low] Copy-paste duplicate assertions in a test
- Location: `types.rs:731-736`
- Category: test hygiene
- Description: The test `token_budget_unenforced_is_visible_in_both_renderers` has two back-to-back identical assertions for `captured.human_summary(1_000).contains("token budget: 50000")`. The first asserts the budget line appears; the second is an exact duplicate with no second assertion between them.
- Evidence:
  ```rust
  assert!(captured
      .human_summary(1_000)
      .contains("token budget: 50000"));
  assert!(captured
      .human_summary(1_000)
      .contains("token budget: 50000")); // ← duplicate
  assert!(!captured.stable_summary().contains("unenforced"));
  ```
- Suggested fix: Delete one of the two identical `assert!` calls. The test's intent is to assert once, then assert `stable_summary` omits "unenforced".
- Risk: Zero — the duplicate makes the test pass twice as hard as it needs to, masking a potential future removal of the first assertion.

---

### [low] `tick_prompt` is 63 lines and mixes three concerns
- Location: `pursuit.rs:123-185`
- Category: function complexity
- Description: `tick_prompt` handles three distinct responsibilities within one 63-line function: (1) last-tick detection and wrap-up prompt, (2) remaining-quota clause construction (iteration / deadline / token budget sub-clauses), and (3) cadence-hint injection. Each sub-clause in the quota block is independently readable but interleaved with the format! calls, and the final format! uses four named parameters that all reference outer variables — a signature that is hard to extend safely.
- Suggested fix: Extract `remaining_quota_clause(state, tokens_now, now_ms) -> Option<String>` and `cadence_hint(state) -> &'static str` as private helpers. The top-level function becomes ~25 lines, dominated by the two early-return branches.
- Risk: Low — pure refactor, no behavioral change. The three current call sites (`try_claim_tick` mod.rs, two tests in pursuit.rs) use it as-is.

---

### [low] Three renderers independently spell out deadline and cadence countdown logic
- Location: `types.rs:354-496` (`human_summary`, `stable_summary`, `live_status`)
- Category: duplication
- Description: The deadline countdown and model-paced next-wake rendering are each written identically in all three renderers:

  **Deadline countdown (deadline_ms present, now_ms != 0, deadline > now_ms):**
  - `human_summary` line 363-366
  - `live_status` line 469-472

  **Model-paced next-wake (model-paced, Some(wake), now_ms != 0, wake > now_ms / due-now / unset):**
  - `human_summary` lines 381-389
  - `stable_summary` lines 448-453 (only the `next wake: ...` line; stable omits the clock-unavailable case)
  - `live_status` lines 477-484

  **Fixed-cadence next-tick (fixed, pending_tick_wake_ms Some(wake), now_ms != 0, wake > now_ms / due-now):**
  - `human_summary` lines 395-404
  - `live_status` lines 489-498

  The only difference is that `stable_summary` elides the clock-unavailable branch for model-paced (always shows "next wake: unset" rather than no line), which is intentional.

- Suggested fix: Extract a shared private helper e.g. `render_next_tick(state, now_ms) -> Option<String>` that produces the next-fire line for both cadences, and call it from `human_summary` and `live_status`. `stable_summary` keeps its own simplified rendering (no countdown at all, but shows "next wake: unset" for model-paced for UX symmetry). Deadline countdown can remain inlined in each renderer given it's only 3 lines — extracting it would add more indirection than it saves.
- Risk: Low — pure extraction, no behavioral change. The current implementations are correct; the duplication is a maintenance hazard only.

---

### [low] `deadline_reached_note` takes `_state` and discards it
- Location: `pursuit.rs:200-202`
- Category: dead parameter
- Description: The function signature is `pub fn deadline_reached_note(_state: &LoopState) -> String` but `state` is never used — the returned string is constant. The prior-art `cap_reached_note` legitimately reads `state.max_iterations` to fill in the actual number.
- Evidence:
  ```rust
  pub fn deadline_reached_note(_state: &LoopState) -> String {
      "Loop stopped: reached its time limit.".to_string()
  }
  ```
- Suggested fix: Change to `pub fn deadline_reached_note() -> String` and update the one call site (`stop_reason_note`). This also makes the asymmetry with `cap_reached_note` explicit — deadline notes are constant because the deadline is already surfaced in the human summary.
- Risk: Zero — single call site in the same file, trivial change.

---

## Cross-cutting

- **No TODO/FIXME**: clean.
- **No dead `pub(crate)` / unconditional `pub` exports**: all `pub` items in `mod.rs` are either the module boundary (`pursuit`, `types`) or the global registry handle — both intentional. `fmt_duration_ms` is `pub` because `goal::pursuit` imports it — legitimately cross-module.
- **No `#[allow]` found**: no unjustified suppressions.
- **No `.clone().with_` outside lock guards**: all cloning is inside `lock()` guards or test helpers — correct.
- **`stop_all` / `transition` / `pause_all_owned_by` all re-implement the "refund unrun tick" logic**: documented as intentional — each has distinct context and the refund call site carries different `reason`/`status` intent. Not extractable without losing that intent.
- **`try_claim_tick` and `rearm_after_interruption` both gate on `pending_tick_wake_ms.is_none()`**: correct — one is claim-time, the other is fire-time. Symmetry is intentional.

## Out of scope

- Prior-review findings (REVIEW.md 2026-08-22): `stop_all` refund, `try_claim_tick` clock-unavailable fail-open, `pause_all_owned_by` atomicity — all resolved in commits 4c0ce96e6 / 9411113da / 3a6be82b5.
- Function-line-count threshold (60L): `tick_prompt` at 63L is the closest; no other non-test function exceeds 50L.
- Lock across `.await`: `MutexGuard` is held only over synchronous in-memory HashMap operations; no `.await` in scope.
- Serialization: the module doc explicitly forbids `Serialize`/`Deserialize` on these types.

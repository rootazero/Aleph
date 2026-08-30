# Looping + Markdown Review (2026-08-28)

**Scope:** `src/looping/` (`mod.rs` 1285L, `types.rs` 871L, `pursuit.rs` 481L); `src/markdown/` (`mod.rs` 6L, `fences.rs` 577L).
**Reviewer:** static, subagent
**Method:** Read every file end-to-end; cross-referenced callers via `grep` (`builtin_tools/loop_manage.rs`, `gateway/formatter/splitting.rs`, `gateway/execution_engine/execute.rs`, `gateway/handlers/users.rs`); compared against `review-results/looping/REVIEW.md` (2026-08-22) and the post-review commits `3a6be82b5` ("looping: review round 3") and `e730f26b7` ("capability: every slot names its own MissingSemantics"). Diffed `3a6be82b5..HEAD` over the two modules. No compile or test runs.

## Summary

- **P0:** 0
- **P1:** 1 (one regression of a class already fixed in another sibling code path)
- **P2:** 0
- **P3:** 0 findings detailed (count: 1 — the multi-`format!` allocation pattern, inherited from prior review).
- **Total findings:** 1 (plus 5 cross-cutting observations).
- **Architecture compliance:** clean. R1 holds (no platform imports in either module: `rg -n "cfg\(target_os|core_graphics|app_kit|objc"` returns 0 matches). R3 holds (no new deps; regex crate already pulled in for fences.rs). No `unwrap`/`expect` in production paths except the one-shot `LazyLock` fence-regex initializer. Mutex poison handled via `unwrap_or_else(|e| e.into_inner())` (project P7).

## Status of 2026-08-22 Review Findings

The diff `3a6be82b5..HEAD` over the two modules is small (~52 insertions, 11 deletions) and almost entirely structural. The actual content churn for `looping/` is concentrated in `3a6be82b5` ("looping: review round 3"); `e730f26b7` only swapped `OnceCell` → `CapabilitySlot<Arc<LoopRegistry>>` and pinned the new `MissingSemantics::ConsumerDecides` variant in a regression test.

| Prior finding (2026-08-22) | Status as of HEAD |
|---|---|
| **[high] `stop_all` no iteration refund on Active→Stopped** | **RESOLVED** — `mod.rs:223-227` mirrors `transition`'s `refund_iteration` step; pinned by `stop_all_refunds_iteration_on_active_with_pending_tick` (mod.rs:1173-1192). |
| **[high] `try_claim_tick` fails closed on `now_ms==0` + pending** | **RESOLVED** — `mod.rs:338` (`now_ms != 0 && now_ms < …`) is now the conjunction, so `now_ms==0` skips the gate; pinned by `try_claim_tick_proceeds_when_clock_unavailable_and_tick_in_flight` (mod.rs:1197-1218). |
| **[medium] `pause_all_owned_by` N+1 lock acquisitions** | **RESOLVED** — `mod.rs:249-285` holds the lock across scan + transition. |
| **[medium] Token budget unenforced never surfaced** | **RESOLVED** — `types.rs:347-355` (human_summary) and `types.rs:382-389` (stable_summary) both print `"declared, unenforced: counter unavailable"`; pinned by `token_budget_unenforced_is_visible_in_both_renderers` (types.rs:776-799). |
| **[medium] `tick_prompt` quota arithmetic `n==max`** | **Still moot.** The `capped` short-circuit returns before the quota clause, so `max - n ≥ 1` always in that branch. |
| **[medium] `tick_prompt` finality over-conservative for ModelPaced** | **Still by design** — doc at `pursuit.rs:74-78` acknowledges it. |
| **[medium] `LoopRegistry` has no eviction of Stopped loops** | **Still no eviction.** Bounded per session; not regressed. |
| **[low] `with_owner_scope` allocations** | Still unchanged; not a regression. |
| **[low] `stop_reason` unbounded `Option<String>`** | Still unchanged; internal callers use short strings. |
| **[low] `tick_prompt` under lock** | Still unchanged; same as prior note. |
| **[low] Test gaps** | **PARTIALLY ADDRESSED** — the three big items from the prior review (stop_all refund, clock-unavailable, missing budget surfacing) now have dedicated regression tests. Concurrent-stress testing and `stop_loop_on_failure`/`park_loop_on_transient_failure` integration are still absent. |
| **[low] `init_global` lazy-init race** | **Resolved by elimination** — the race-window `Arc::new(...)` pattern is now confined to test bootstrappers (`gateway/handlers/{users,kanban}.rs` and `db_handlers/modify.rs`), not production. `e730f26b7`'s `CapabilitySlot` swap leaves the same single-thread lazy-init contract in tests. |

Test count rose from ~82 (per prior review) to 115 (`#[test]` totals: mod 32, pursuit 26, types 28, fences 29).

## Findings

### [P1] `looping/mod.rs:249-285` — `pause_all_owned_by` does not refund iteration when pausing an Active loop with a claimed-but-unfired tick

- **Category:** logic / data integrity
- **Confidence:** High (~90%)
- **Description:** `pause_all_owned_by` walks every `Active`/`Paused` row owned by `user_id`, then writes the paused state directly with `with_status(Paused).with_stop_reason(...).with_pending_tick(None)`. It never calls `live.clone().refund_iteration()` before clearing `pending_tick_wake_ms`. The sibling code paths do:
  - `transition` (`mod.rs:148-191`): Active→Paused with `pending_tick_wake_ms.is_some()` → refunds via `retires_unrun_tick`.
  - `stop_all` (`mod.rs:203-220`, fixed in `3a6be82b5`): refunds via the same `retires_unrun_tick` test.
  - `commit_field_update` (`mod.rs:469-475`): refunds when `reschedule=true && live.pending_tick_wake_ms.is_some()`.

  `pause_all_owned_by` is the deactivation-freeze path (`freeze_owned_background_work` in `gateway/handlers/users.rs:685-727`), invoked on account deactivation. A paused-then-resumed loop whose cap was hit while paused-and-paused-because-of-deactivation leaves `iterations_used` carrying the count for an unrun tick. For a small `max_iterations` (e.g. `max=1`), this turns a single unrun tick into "loop stopped: reached the iteration cap (1 ticks)" after the user resumes.

- **Impact:**
  1. Counter accounting is inconsistent with `transition`/`stop_all`/`commit_field_update` after the same kind of supersede — exactly the bug class `3a6be82b5` explicitly closed elsewhere in this module.
  2. The user-facing `human_summary`/`stable_summary` reads `iterations_used: N/max` after resume and may report a cap-reached stop that never corresponded to an executed tick.
  3. The deactivation freeze is the user-facing `freeze_owned_background_work` warning ("deactivation paused owned loops") path. The shipped receipt conflates "we paused your loops" with "we silently shifted your iteration cap"; on resume the model may then spuriously claim "you ran out of ticks" on a watch that never ran.

- **Suggested fix:** Route the per-row write through the documented contract. Two equally valid shapes:

  (a) Mirror the `transition` body verbatim inside the loop:

  ```rust
  let next = {
      let retires_unrun_tick = from == LoopStatus::Active && live.pending_tick_wake_ms.is_some();
      let base = if retires_unrun_tick {
          live.clone().refund_iteration()
      } else {
          live.clone()
      };
      base.with_status(LoopStatus::Paused)
          .with_stop_reason(Some(reason.clone()))
          .with_pending_tick(None)
  };
  ```

  (b) Refactor to actually call `transition` and map the result — matching the doc's own claim ("going through `transition` — THE single atomic lifecycle move"). This also picks up any future legality-matrix changes for free:

  ```rust
  match self.transition(session_id, LoopStatus::Paused, Some(reason.clone())) {
      TransitionOutcome::Applied { from: LoopStatus::Active } => applied += 1,
      TransitionOutcome::Applied { .. } => {} // was Paused, no count
      _ => continue,
  }
  ```

  Option (b) eliminates the hand-rolled `legal = matches!(from, Active | Paused)` check (mod.rs:268), which is correct today but drifts from the matrix in `transition`. Add a regression test analogous to `stop_all_refunds_iteration_on_active_with_pending_tick`, plus a no-refund-on-already-paused assertion:

  ```rust
  #[test]
  fn pause_all_owned_by_refunds_unrun_ticks() {
      let reg = LoopRegistry::default();
      let alice = crate::scope::ScopeAttribution::personal("u-alice");
      let mut s = st("s-alice").with_owner_scope(Some(&alice)).with_max_iterations(Some(1));
      reg.put(s.clone());
      // Claim a tick → iterations_used=1, pending=Some(_).
      let _ = reg.try_claim_tick("s-alice", None, 1_000);
      assert_eq!(reg.get("s-alice").unwrap().iterations_used, 1);
      let count = reg.pause_all_owned_by("u-alice");
      assert_eq!(count, 1);
      let after = reg.get("s-alice").unwrap();
      assert_eq!(after.status, LoopStatus::Paused);
      assert_eq!(after.iterations_used, 0,
          "an unrun tick must not survive into the frozen state");
      assert!(after.pending_tick_wake_ms.is_none());
  }
  ```

- **Related:** Same family as `3a6be82b5`'s `stop_all` fix (commit message lists H1). The doc comment at `mod.rs:231-233` already names `transition` as the canonical lifecycle move but the implementation hand-rolls it.

## Cross-Cutting Observations

1. **MissingSemantics pin (e730f26b7).** The new `static GLOBAL: CapabilitySlot<Arc<LoopRegistry>> = CapabilitySlot::new("looping/registry", MissingSemantics::ConsumerDecides)` (mod.rs:556) is correct and the regression test at `the_registry_slot_pins_its_missing_semantics` (mod.rs:1275-1281) pins it. Sharpest consumer (`execute.rs:1535`): `let confirmed = crate::looping::global().is_some_and(|reg| reg.confirm_fire(..))` — missing registry ⇒ `confirm_fire` returns `false` ⇒ tick dropped with the log "loop: tick superseded during its delay (loop stopped or replaced)", which is a confident explanation of something that did not happen. This is the exact behavior `ConsumerDecides` is named for. Not a finding — observation that the new pin is well-grounded.

2. **`pause_all_owned_by` doc/code mismatch.** Even aside from the iteration refund, the doc at `mod.rs:230-234` says "going through `Self::transition`] — THE single atomic lifecycle move (CLAUDE.md A4) — instead of hand-rolling a second one", but the body (mod.rs:249-285) does not call `transition`. This invites drift (e.g., the next legality-matrix change in `transition` won't reach here). The fix in finding 1 above doubles as closing this documentation lie.

3. **`confirm_fire` "stolen wake" race.** In a contrived pause→resume sequence where the user pause happens between a tick's claim and confirm_fire, the (now-paused) tick's `confirm_fire(wake_ms)` will mismatch the cleared marker and correctly refuse. **However**, if the user resumes and the continuation hook re-claims with the *same* wake time (fixed cadence, same `now_ms` window), the live marker equals the old wake. The old confirm_fire then matches, clears the marker, and runs the OLD prompt — the new claim's pending marker is consumed. iterations_used stays correct (refund on pause, +1 on new claim, +0 on rearmed tick's run), so no over-count; only the prompt that ran is the previous one. This is documented as the design ("`confirm_fire` clears the marker in the same lock guard"), the cost is one confused tick body, and it's not worth gating on. Note as observation; no action.

4. **`tick_prompt` quota clause on counter reset.** `pursuit.rs:155-159` prints `" ~{budget} of {budget} token budget left"` when the session counter went backwards (the saturating-sub means `tokens_used = 0`). Documented trade-off (`types.rs:256-258`); not a regression.

5. **`FenceSpan` API surface.** `FenceSpan::start/end` are `pub(crate)` (fences.rs:19, 21); only `splitting.rs` consumes them via accessors. `FenceSpan::contains` (fences.rs:89-90) and the matching `is_safe_fence_break`/`find_fence_at`/`get_fence_split` form a complete and minimally-exposed contract. The doc at fences.rs:75-87 explicitly hands the "closing marker bytes are outside the span" subtlety to `get_fence_split`, which is the only correct path — and `splitting.rs:73` is the only caller. No second backtick-counting heuristic in scope (the comment at splitting.rs:7-9 makes this a deliberate architectural choice). `markdown_to_platform.rs` / `replace_pre_code_blocks` / `parse_markdown_blocks` (formatter/helpers.rs:215, 328) are HTML-side parsers, not fence-streaming splitters, so no shared-parser migration is in flight.

6. **Test-density growth.** 115 tests across 4 files (~32 in mod, ~26 in pursuit, ~28 in types, ~29 in fences). Coverage of the cap-reach / clock-unavailable / budget-capture / lifecycle matrices is thorough. The one regression test gap that maps to the finding above is exactly the missing `pause_all_owned_by_refunds_unrun_ticks` test noted in the fix.

## Out of Scope / Skipped

- **`loop_graph/`, `goal/`, `cron/`** peer subsystems: parallel APIs, not under review here. The diff `3a6be82b5..HEAD` does not touch the parallel `goal::store::pause_all_owned_by` or `cron::pause_all_owned_by` paths; only `looping/` changed content.
- **`gateway/execution_engine/execute.rs::try_claim_tick` / `confirm_fire` / `stop_loop_on_failure` / `park_loop_on_transient_failure` orchestration**: referenced by callers, not reviewed here.
- **`builtin_tools/loop_manage.rs` full audit**: only the `start`/`resume`/`update`/`pause` and the lazy-init helper paths were touched in this review.
- **`loop_graph` ↔ `looping` boundary**: clean; `loop_graph/service.rs` is not in this review's surface.
- **Live `cargo check`/`clippy`/`test`**: explicitly skipped per directive.
- **Stress / concurrency testing**: no multi-threaded test exists for the registry. The finding above is the only concrete regression that the missing test would have caught.
- **Memory-leak / long-running-daemon behavior**: `LoopRegistry` has no eviction. Same as prior review; bounded but unquantified.
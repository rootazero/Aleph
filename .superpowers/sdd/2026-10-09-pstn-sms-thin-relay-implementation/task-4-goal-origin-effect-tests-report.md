# Task 4 — goal-origin effect-level tests (review I1)

Base: 78d39cf5a. Test-only: no production code changed.

## Gap (I1)
`goal_origin_race.rs` proves post_run / wake under an awaited token read. It did
not prove, at the engine level and by EFFECT, that a stale origin cannot run or
consume state at: fire (`confirm_fire_for_origin`), busy re-arm
(`rearm_goal_after_busy`), gate veto (`claim_after_gate_veto` via
`arbitrate_gate`), boot re-arm (`rearm_parked_goals`).

## Added: `src/gateway/execution_engine/tests/goal_origin_effects.rs` (9 tests)
Each mismatch test runs 4 swaps: public->ordinary, ordinary->public,
public A->public B, public->unreadable marker. Each has a same-origin control.
Effect = a `RunRequest` did/did not reach a real `ExecutionAdapter`, and the
goal row's marker / status / counters.

| Seam | Real path driven | Mismatch assertion | Control |
|---|---|---|---|
| fire | `spawn_continuation_run` Goal arm after a real bound claim | no request within 700ms; `pending_continuation_ms == Some(wake)` kept; `continuations_used` unchanged | request carries its origin; marker consumed |
| busy re-arm | adapter returns `AgentBusy` after the origin is swapped inside the busy window | no marker re-stamped (`None`), adapter called once, no extra iteration | marker re-stamped |
| gate veto | `post_run` with a real `StopHookHandler` that swaps the origin then Blocks (slot free during gate) | no request; goal stays `Complete` (not reopened); `continuations_used == 0` | request under the seeded origin; goal reopened |
| boot re-arm | `GoalWakeService::rearm_parked_goals` | unreadable origin: no run, marker kept. Boot->fire race: origin swapped after re-arm, before the delayed fire -> no run, marker kept | public / ordinary / legacy respawn under stored origin, marker consumed |

## Test seam (test-only)
`tests.rs`: `mod goal_origin_effects;` and a static mutex serializing
`goal_store_global()`'s open+install. Without it parallel tests re-opening the
same sqlite file flaked with `goal store open: database is locked` (observed
~1/3 runs once more tests used it). After the fix: 6/6 consecutive green runs
of `goal_origin` (24 tests).

## Verification (actually run)
- `cargo test -p alephcore --lib goal_origin`: 24 passed (9 new + 15 existing), x6 runs.
- Mutation check: temporarily replaced the three `live_origin_is(..)` calls in
  `goal/store.rs` (claim_after_gate_veto, confirm_fire_for_origin,
  rearm_after_busy) with `true`: fire, busy, gate-veto and boot->fire mismatch
  tests turned red; restored (store.rs unchanged in the commit).
- `rustfmt --edition 2021 --check` on the new file clean; `git diff --check` clean.
- `cargo test -p alephcore --lib gateway::execution_engine`: 476 pass, 2 fail
  (`btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`,
  `run_loop::tests::project_skill_block_none_when_no_project_skills`) — both
  fail identically on a clean HEAD (stash-verified); unrelated.

## Boundaries / not done
- Boot corrupt-marker test stays green under the mutation by design: that guard
  is `bound_origin` (not `live_origin_is`), already covered by its own assertion.
- Boot re-arm replacement is exercised via the fire guard (1s real delay), not by
  injecting between `bound_origin` and spawn inside `rearm_parked_goals` — there
  is no await seam there without changing production code.
- Busy-retry respawn (30s `BUSY_RETRY_DELAY_MS` sleep) is not awaited; only the
  re-arm decision / marker is asserted.
- Real-shell `gate_command` path not used; the gate is a `StopHookHandler` stub
  (same trait object `arbitrate_gate` runs).
- Source-grep census tests left untouched; no coverage deleted.

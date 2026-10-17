# Task 4 fix — bind the autonomous telecom origin to the claimed goal

Branch `feat/pstn-sms-thin-relay`, base `ae5c47912`.
Status: **partially complete** (see "Not done / residual"). Not claimed safe-complete.

## Finding

`goal_continuation::post_run` peeked the goal, resolved its creator telecom
origin from that snapshot, `await`ed `live_tokens`, then claimed through the
session-only `try_claim_continuation`. A goal replaced inside the await (public
-> ordinary, ordinary -> public) was continued with the stale origin. The wake
service (`goal_wait::claim_and_spawn`) had the same snapshot -> await -> claim
gap, and resolved the origin even later (after the claim). The rearm paths
(`rearm_after_busy`, `claim_after_gate_veto`) re-spawned with the origin captured
at the first claim.

## Fix

Origin is now read in the SAME critical section (one store lock) as the goal row
it belongs to, and compared to what the caller is about to run.

`src/goal/store.rs`
- `try_claim_continuation_bound(expected: &Goal, ...) -> BoundClaim`:
  under one lock, live goal must be the same goal (id + owner + scope + author)
  else `Replaced` (nothing written); an unreadable/mismatched origin marker is
  `Refused` (nothing written); otherwise claims and returns
  `Claimed { decision, origin }` where `origin` is the LIVE goal's creator origin.
  Callers spawn from that origin only. Unbound `try_claim_continuation` stays
  (tests / store-internal), doc-warned, and a census forbids production callers.
- `bound_origin(&Goal) -> BoundOrigin` for the boot re-arm of an already-claimed
  wake (no claim to bind to).
- `rearm_after_busy(session, now, expected_origin)` and
  `claim_after_gate_veto(.., expected_origin)`: retry only while the live goal's
  origin equals the origin the run was started with; otherwise `Drop`/`None`.
- `confirm_fire_for_origin(session, wake_ms, now, run_origin)`: the fire-time
  confirm now also requires the live creator origin to equal the origin the run
  carries; mismatch => `Superseded`, marker not consumed. `confirm_fire` is the
  same logic without the origin check (used by the out-of-bounds arbiter that
  spawns no run).

`src/gateway/execution_engine/`
- `goal_continuation.rs`: `post_run` claims with the bound claim, uses the
  returned origin; `arbitrate_gate`/`rearm_goal_after_busy` thread it through;
  `resolve_goal_origin` removed.
- `goal_wait.rs`: `claim_and_spawn` uses the bound claim; `spawn_wake_run` takes
  the origin from the claim; boot re-arm uses `bound_origin`.
- `execute.rs`: goal confirm uses `confirm_fire_for_origin`; rearm call sites
  pass the captured origin.

Invariant: a continuation runs only under the creator origin of the goal that was
claimed/confirmed in the same store critical section; any change of goal id or
creator origin between claim and run drops the run (fail closed), it is never
re-labelled with another goal's origin.

Design choice stated, not hidden: a same-id replacement by the same creator
(same objective re-set) is the "same goal" by `same_goal`; it continues under the
LIVE origin (which is correct for it). Origin equality, not timestamps, decides
whether a retry/replay may proceed.

## Tests (all run)

- `cargo test --lib goal`: first run 262 passed, 1 failed (an existing
  test, `goal_continuation_inherits_the_originating_runs_public_telecom_context`,
  that seeded a legacy origin-less goal with a plain `put` and ran it as public;
  the new fire-time origin check correctly supersedes that inconsistent state.
  Fixed the fixture (goal created by that public run via `put_with_origin`);
  assertions unchanged. Re-run: 263 passed, 0 failed.
- Store (`goal::store`, 64 passed): bound claim returns live origin after same-id
  replacement; replaced/cleared => `Replaced` without writing; untrustworthy
  origin => `Refused` without writing; rearm and gate-veto origin equality;
  `confirm_fire_for_origin` supersedes on origin mismatch.
- Engine, new `src/gateway/execution_engine/tests/goal_origin_race.rs` (15 passed):
  a `SessionStore` decorator replaces the goal inside the REAL awaited token read
  (`get_total_tokens`, reached by budgeted goals via `live_tokens`); each case
  asserts the read happened exactly once.
  - post_run: public->ordinary => `telecom_public == None`; ordinary->public,
    legacy->public, public A->public B => live context; different goal / cleared
    / corrupted origin => no run, no iteration spent, no pending marker; controls
    (public/ordinary/legacy, no replacement) unchanged.
  - wake (`GoalWakeService::sweep_once` on an elapsed timer barrier): same
    matrix (public<->ordinary, different goal, corrupt origin, controls).
  - census: no production caller of unbound `try_claim_continuation` outside
    `goal/store.rs`; `execute.rs` goal confirm is the origin-bound one.
- Mutation check (not committed): making the bound claim skip the same-goal check
  and return a stale `None` origin turned 10 of the 15 engine tests red; restored.
- `cargo test --lib gateway::execution_engine`: 467 passed, 2 failed, both
  environmental and in files not touched: `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`
  (skills/plugins git submodules not initialised in this checkout) and
  `run_loop::tests::project_skill_block_none_when_no_project_skills`.
  Not verified against a clean baseline build.
- `cargo check --lib --tests` clean (only pre-existing warnings); `git diff --check`
  clean; `rustfmt --edition 2021 --check` clean on all touched Rust files.
  cargo ran with `CARGO_BUILD_JOBS=2 OPENSSL_NO_VENDOR=1`; no live/provider use.

## Not done / residual

- **Loop path not fixed (registered, not expanded):** `rearm_loop_after_busy`
  (`execute.rs`, in-memory `LoopRegistry`) and the loop `telecom_origin` snapshot
  have the analogous snapshot -> re-run shape. Untouched by instruction.
- Boot re-arm read of an already-claimed wake (`bound_origin`) is not bound to a
  claim; it is backstopped by the marker-keyed `confirm_fire_for_origin`.
- Retry/rearm after `AgentBusy`/authority-Unknown compares captured origin to
  live origin; it does not re-resolve an origin for a different goal.
- The post_run Idle/gateless-complete/settle arms still use the pre-claim peek for
  their notifications (no run is spawned from them).
- Two pre-existing environmental test failures above; no clean-baseline rerun.
- No real-device/live provider verification; no `src/harness/**`, `ScopeId` or
  ordinary behaviour changes.
- Pre-existing uncommitted `task-3c-report.md`, the plan doc and the progress file
  were left unstaged.

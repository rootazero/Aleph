# Task 4 — loop telecom origin bound to the live tick

Commit: `fix: bind loop telecom origin to live tick` (on top of 4b26f25ae).

## Problem
`loop_post_run` snapshotted `get_active` -> awaited `get_total_tokens` -> `try_claim_tick`, then spawned
the continuation with the SNAPSHOT's `telecom_origin`. A loop stopped/restarted/replaced (possibly under
another origin) inside the await was claimed and run under the stale origin. `rearm_loop_after_busy` /
transient park / fire-time `confirm_fire` re-used that captured origin and were keyed only by session
(+ wake marker), so a replacement with the same marker was accepted.

## Fix
- `LoopState.instance: u64` — monotonic, non-clock identity minted in `LoopState::new` (new loop = new
  instance; pause/resume/update/claim keep it; `commit_field_update` preserves the live one).
- Registry (`src/looping/mod.rs`): `try_claim_tick_bound(expected, ..) -> BoundTick::{Claimed{decision,origin,instance}, Replaced}`
  checks active + same instance under the ONE lock and returns the LIVE row's creator origin;
  `confirm_fire_for`, `rearm_after_busy_for`, `rearm_after_interruption_for` are instance-bound. Unbound
  variants kept for other callers.
- `execute.rs`: loop hook extracted to `loop_post_run`; spawns with the origin returned from the claim;
  `Replaced` => log + drop, no write. `ContinuationKind::Loop { wake_ms, instance }`; busy, authority-unknown
  and transient paths pass the instance and drop (None) if the incarnation changed. No timestamps used.
- Restart during the gate is a NEW incarnation => dropped (fail-closed), not continued under the new origin.
  Same-incarnation edits (field update, pause+resume) still run under the creator origin.

## Tests (`src/gateway/execution_engine/tests/loop_origin_race.rs`, real adapter, no real waits)
post-run gate (public->ordinary, ordinary->public, A->B restart => adapter NOT run, pending/iterations/status
untouched; stop/pause dropped; replacement later claims under its own origin; controls: nothing / field update /
pause+resume x public/ordinary run under creator origin), fire-time (claimed incarnation runs + consumes marker;
replacement carrying the same wake marker is not fired and its marker kept), busy (same incarnation re-arms;
restart/stop => no retry marker, no extra iteration, no second adapter call), `rearm_loop_after_busy` pure seam,
and a census that `execute.rs` production code calls only the bound registry methods.
`goal_origin_race.rs` only had `Hook`/`ReplaceOnTokenRead` made `pub(super)`.

## Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo test --lib loop_origin_race`: 9 passed.
- `cargo test --lib -- looping:: execution_engine:: goal_origin loop_manage`: 617 passed, 2 failed, both
  unrelated: `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` (known baseline red) and
  `run_loop::tests::project_skill_block_none_when_no_project_skills` (project-skill scan of a tempdir; not
  loop code; not re-verified against a clean HEAD).
- rustfmt --edition 2021 clean on touched files; git diff --check clean.

## NOT done / residual
- RED-first was not literally followed: implementation was written before the tests; no mutation run
  (making the bound calls ignore `instance`) was performed to demonstrate RED.
- No dedicated unit tests inside `src/looping/mod.rs` for the new registry methods (covered via the
  engine-level tests only).
- (Historical, as of this section) `halt_on_refused_authority` paused the loop through the unbound `transition`,
  so a stale tick could pause a replacement loop; its test used `Loop{wake_ms:0,instance:0}`. **Fixed by
  Follow-up commit 6a4927174** (now uses `transition_for`); see the "Follow-up" section below for the
  verification and the remaining residuals.
- The origin carried to the run is still read at claim time; origin captured after `confirm_fire`/execute is
  inherent (as in the goal side).
- Loop registry is process-memory only; instance ids reset on restart (nothing persisted).
- `park_loop_on_transient_failure` instance binding is only covered by the updated existing unit tests, not a
  new awaited-boundary test.
- No change to src/harness/**, ScopeId, ordinary channels, Goal logic; no consumers/private/approval/live.
- Not committed: task-3c-report.md, the plan, progress.

---

# Follow-up — fence stale loop stop effects

Commit: `fix: fence stale loop stop effects`.

## Problem (independent review, Important)
`ContinuationKind::Loop` carried `instance`, but three stale-tick effects in `execute.rs` still went through the
unbound `transition`, so an old tick resuming after an await could stop/pause a REPLACEMENT loop:
agent-missing branch (Stopped + `clear_loop_welded_strategy`), `halt_on_refused_authority` (Paused + `notify_origin`),
`stop_loop_on_failure` (Stopped + strategy clear + notify).

## Fix
- `src/looping/mod.rs`: `transition` now delegates to private `transition_inner(session, expected_instance: Option<u64>, ..)`;
  new `transition_for(session, instance, to, reason) -> TransitionOutcome`. The instance is checked under the same
  lock as the legality matrix; mismatch (or no active loop) => `Refused{current}`, nothing written. Unbound `transition`
  keeps its old behaviour for other callers. Fence is instance-only (not Active-required) so a same-incarnation
  Paused loop can still be stopped by a failure; the legality matrix still applies.
- `execute.rs`: new `halt_loop_on_agent_miss(session, instance, agent_id) -> bool`; `halt_on_refused_authority` Loop arm and
  `stop_loop_on_failure(session, instance, err, origin)` use `transition_for`. Strategy clear and user notification
  happen ONLY when the outcome is `Applied` (previously `stop_loop_on_failure` notified unconditionally).

## Tests (`loop_origin_race.rs` section 5, real registry/strategy/adapter effects)
Per class a negative (replacement untouched: status, iterations, strategy kept, nothing notified) and a positive
(same incarnation changes + strategy cleared/kept as designed): `transition_for_is_fenced_to_the_incarnation`,
`agent_miss_stop_*`, `refused_authority_*`, `failure_stop_*`; e2e: `a_tick_whose_agent_is_gone_stops_its_own_loop`,
`a_failed_tick_stops_its_own_loop` (control) paired with `a_failed_stale_tick_does_not_stop_the_replacement`
(real race via replace-during-run adapter + quiet wait, not a sleep-and-hope). Census now forbids `.transition(`
and requires `.transition_for(` in `execute.rs` production code.

## Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo test --lib loop_origin_race`: 19 passed.
- Mutation: disabling the instance fence in `transition_inner` (`if false && ..`) => 5 FAILED (agent_miss negative,
  failure_stop negative, refused negative, transition_for fence, a_failed_stale_tick). Reverted; tests green.
- `cargo test --lib -- looping:: execution_engine:: goal_origin loop_manage`: 627 passed, 2 failed, both unrelated
  and pre-existing: `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`,
  `run_loop::tests::project_skill_block_none_when_no_project_skills`.
- rustfmt --edition 2021 --check clean on the 3 touched files; `git diff --check` clean.

## NOT done / residual
- `halt_on_refused_authority` and agent-miss negatives are exercised at the seam only (authority refusal cannot be driven
  end-to-end cheaply; no hook between `confirm_fire` and the registry lookup). The e2e race is covered only for the failure path.
- User notification is not asserted (tests run with origin None).
- The two unrelated reds above were not re-verified against a clean HEAD this round.
- No change to src/harness/**, ScopeId, consumers/private/live. Not committed: task-3c-report.md, plan, progress.

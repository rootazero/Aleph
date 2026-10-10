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
- `halt_on_refused_authority` still pauses the loop through the unbound `transition`: a stale tick can pause
  a replacement loop. Its test uses `Loop{wake_ms:0,instance:0}`.
- The origin carried to the run is still read at claim time; origin captured after `confirm_fire`/execute is
  inherent (as in the goal side).
- Loop registry is process-memory only; instance ids reset on restart (nothing persisted).
- `park_loop_on_transient_failure` instance binding is only covered by the updated existing unit tests, not a
  new awaited-boundary test.
- No change to src/harness/**, ScopeId, ordinary channels, Goal logic; no consumers/private/approval/live.
- Not committed: task-3c-report.md, the plan, progress.

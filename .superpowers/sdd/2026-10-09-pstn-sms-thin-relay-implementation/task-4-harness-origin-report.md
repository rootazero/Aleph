# Task 4 — harness spawn carries the telecom run origin

Branch `feat/pstn-sms-thin-relay`, base d73adf789.

## Problem (review Important)
`run_loop::with_request_scope` seeds the run-origin task-local
(`gateway::telecom::run_origin`). `Orchestrator::dispatch`
(`src/orchestrator/dispatch.rs`) then `tokio::spawn`s the harness; task-locals
do not cross a spawn, so a tool running inside the harness task
(`sessions.send`, `goal`, `loop`) saw `current_run_origin() == None` and the
d73adf789 sessions provenance fix was inert in production (public run ->
ordinary child).

## Change
- `dispatch.rs`: capture `req.telecom_public.clone()` before the spawn
  (sixth capture, next to the others) and wrap
  `with_transcript_source(.., harness.run(..))` in
  `run_origin::seed_run_origin(captured, ..)`. Source is the request's typed
  field only — never metadata / `source_channel`. `None` seeds explicitly as
  ordinary (also clears any stale ambient origin); nothing is invented.
  `seed_run_origin` sits outside `with_transcript_source` so the existing
  `the_harness_spawn_carries_the_transcript_source` guard still holds.
- `run_origin.rs`: doc M1 fixed (known-gap pointer now names
  `task-4-sessions-provenance-report.md` / this report); doc and census test
  now state two production callers (`run_loop/mod.rs`, `orchestrator/dispatch.rs`)
  — `seed_run_origin_has_exactly_two_production_callers`.

## Tests
- `orchestrator/tests/dispatch.rs::the_harness_spawn_seeds_run_origin_from_the_typed_request_field`:
  probe harness inside the real spawn; public request -> ambient Some and typed
  Some; ordinary request under a stale seeded origin -> ambient None, typed None.
- `builtin_tools/sessions/send_tool.rs::sessions_send_inside_the_harness_spawn_carries_the_requests_origin`:
  real `Orchestrator::dispatch` -> harness spawn -> real `SessionsSendTool::call`
  -> adapter -> child `RunRequest.telecom_public` (both the initial and the
  `-continue` run) equals parent `Some(ctx)` / `None`.
- Red first: with dispatch.rs at HEAD both fail (`ambient: None`, `left: None`);
  green with the fix. Existing scope-carrier / transcript-source / run-tree
  originator guards unchanged and green.

## Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo test -p alephcore --lib -- the_harness_spawn sessions_send_inside orchestrator:: sessions:: telecom:: hook_transcripts source_guard scope::` -> 570 passed, 0 failed.
- `cargo clippy -p alephcore --lib --tests`: no diagnostics in touched files.
- `rustfmt --edition 2021 --check` on the 4 files ok; `git diff --check` ok.

## Not done
- Other bare `tokio::spawn`s were not audited for the same task-local loss.
- Full `alephcore` lib suite not run (targeted subsets only); no workspace/cross-crate build.
- No live / provider / real-device run.
- Census test only counts `seed_run_origin(` callers; it does not prove each
  boundary seeds the right value (that is the e2e tests' job).
- `harness/**` untouched; pre-existing dirty docs (task-3c report, plan, progress.md, task-4-loop-origin-fix-report.md) deliberately not committed.

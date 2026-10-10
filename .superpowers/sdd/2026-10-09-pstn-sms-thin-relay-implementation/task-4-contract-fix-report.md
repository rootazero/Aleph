# Task 4 contract fix report — preserve telecom public context across derived runs

Base: `7514f4de8 feat: add telecom public context contract`. Fixes independent-review I1, I2, M1.

## Changes
- **I1 steering fold** (`src/gateway/execution_engine/steering.rs`): `BusySibling` now carries the sibling's `telecom_public` (read in the single `find_busy_sibling` snapshot). `fold_is_admissible` defers when `request.telecom_public.is_some() != sibling.telecom_public.is_some()` (presence only; Some/Some and None/None fold as before). Test: `a_steer_never_folds_across_a_public_telecom_presence_mismatch` (both directions deferred, same-kind unchanged).
- **I2 continuations** (`execute.rs`, `goal_continuation.rs`): `continuation_request` and `spawn_continuation_run` take `telecom_public: Option<TelecomPublicContext>` instead of hardcoding `None`. Propagated along the real chain: run-completion hook → `goal_continuation::post_run` → `arbitrate_gate` / goal continuation spawn; loop-tick spawn; authority-unknown re-arm respawn; AgentBusy / transient-park respawn (`retry_telecom_public`). Tests: `the_continuation_request_inherits_the_originating_public_telecom_context` (builder + fire-time gate keeps it, None stays None); `goal_continuation_inherits_the_originating_runs_public_telecom_context` (reaches the adapter).
- **M1 docs** (`execution_engine/mod.rs`, `telecom/public_context.rs`): "cannot forge" reworded to: only `pub(crate)` constructors; the SMS adapter (typed `MessageMeta::Sms`, stamped in `inbound_router::executor`) is the only production producer; an in-crate convention, not a cryptographic/unforgeable guarantee.

## Verification
- `rustfmt --edition 2021` on the 7 changed `.rs` files only.
- `cargo test -p alephcore --lib --no-run` builds (only pre-existing warnings).
- `cargo test --lib -- gateway::execution_engine gateway::telecom gateway::inbound_router`: 545 passed, 2 failed (below). New tests pass.
- Earlier filtered run (`steering telecom continuation execution_engine::execute inbound_router::executor`): 118 passed, 0 failed.
- `cargo clippy -p alephcore --lib --tests`: no finding in changed code. One `needless_borrow` at `public_context.rs:188` is in the already-committed autoref probe test (7514f4de8); left alone because the borrow is what makes the probe work.
- 2 failures, unrelated to this change by inspection, NOT re-run on a clean baseline: `btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` (its message says the `skills/` and `plugins/` submodules are not initialised in this checkout); `run_loop::tests::project_skill_block_none_when_no_project_skills` (project-skill discovery from the tempdir anchor; touches no code changed here).

## Residual gaps (still `telecom_public: None` — registered, not fixed)
- `goal_wait.rs` `spawn_wake_run` (hook-less goal wake): rebuilt from the goal row, which does not persist provenance. Passes `None` with a comment.
- `busy_queue/durable.rs` reinjection and `resume_coordinator.rs` crash resume: rebuilt from storage.
- `sessions_send` (`send_tool.rs`, incl. continue_request), `announce_delivery`, teams, a2a, cron/heartbeat, openai_api, execution_adapter, handlers: derived/other producers not in this slice.
- These must be closed (provenance persisted as a server-authored marker) before any consumer relies on `Some`.

## Not done
- No consumers, isolation, approval, storage/persistence, real telecom lines.
- No change to `src/harness/**`, `ScopeId`, task-3c report/plan (left unstaged).
- Presence-only fold rule: no comparison of provenance contents between two public runs.
- Did not run the full test suite or a clean-baseline run of the 2 failing tests.

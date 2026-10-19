# Task 5 integration and acceptance report

- Worktree: `D:/Workspace/Aleph-model-hot-switch`
- Branch: `feat/model-hot-switch`
- Spec: `docs/superpowers/specs/2026-10-18-model-hot-switch-design.md`
- Plan: `docs/superpowers/plans/2026-10-18-model-hot-switch.md`
- Acceptance date: 2026-10-18

## Implemented boundary matrix

| Boundary | Implemented behavior | Evidence |
|---|---|---|
| Session selection writer | `select_model` is the authoritative writer through `update_session_model`; CLI/TUI and Panel use the `/model` gateway path. Ordinary `sessions.patch` rejects model pin and generation keys. | `builtin_tools::select_model` 18 tests; session modify 17 tests; Panel model picker 6 tests |
| Durable ordering | Selection updates carry a monotonic generation. SQLite/File stores conditionally merge only a strictly newer generation; stale, malformed, and generation exhaustion cases fail closed. Clear writes a generation tombstone. | Task 1 commit `4061ee89c`; provider/session handle 16 tests; session pin 6 tests; backend generation tests from Task 1 |
| Request boundary | `SessionModelProvider` resolves the current session snapshot at the beginning of each `process`/stream call and delegates the call to an immutable target. A request already in flight is unaffected by later selection changes. | `providers::session_model_provider` 9 tests |
| Precedence | Explicit turn override > session selection > agent hint > flow `BrainRef`. Explicit turn selection bypasses the session wrapper. | `orchestrator::harness_bridge::runner_impl` 19 tests |
| Provenance | `AssistantMessage` has optional per-call model/provider/generation fields with serde defaults. Think and grace paths record the provider target after completion; legacy events remain readable. | `session::events` 31 tests; `harness::agent::think` 13 tests |
| Selection durability readiness | Production selectable resolution refuses when the session pin sink is absent; test-only task-local publications bypass gateway boot so unit tests remain isolated. The control update awaits the generation-conditional store write and rolls back the live selection when persistence rejects or fails. | `resolve_selectable_model` / `update_session_model_durable` / `pin_sink_installed` |
| Command/plugin declared models | `slash_command_body::command_model_pin` uses the same selectable resolver and returns a qualified override; unknown/unavailable/ambiguous/unpublished ids no longer become raw overrides. | `gateway::execution_engine::slash_command_body` 17 tests |
| Panel | Per-turn picker behavior remains separate. Session action sends `/model provider/model` through `ChatApi::send`, binds the returned run, and never writes pin metadata directly. | `cargo check -p aleph-panel --lib`; `model_picker` 6 tests |
| Context and recovery | Existing compaction and run envelope/crash-resume paths remain in place; provenance is optional and excluded from prompt reconstruction. | event round-trip tests and runner snapshot tests |

## Actual verification

All commands below were run from `D:/Workspace/Aleph-model-hot-switch`.

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `CARGO_BUILD_JOBS=2 cargo check -p alephcore --tests` — passed. Existing warnings remain in test-only code; no compile errors.
- `CARGO_BUILD_JOBS=2 cargo clippy -p alephcore --all-targets` — passed. Existing test-only unused/dead-code warnings remain; no feature clippy warning remains after allowing the intentional trait-object constructor.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore gateway::execution_engine::slash_command_body --lib` — 17 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore builtin_tools::select_model --lib` — 18 passed after switching the control write to the durable async seam.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore gateway::session_model_pin::tests --lib` — 6 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore providers::session_model_handle --lib` — 16 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore providers::session_model_provider --lib` — 9 passed after recording provenance only after successful provider completion.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore orchestrator::harness_bridge::runner_impl --lib` — 19 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore harness::agent::think --lib` — 13 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore session::events --lib` — 31 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore gateway::session_model_pin::tests --lib` — 6 passed.
- `CARGO_BUILD_JOBS=2 cargo test -p alephcore gateway::handlers::session::db_handlers::modify::tests --lib` — 17 passed.
- `CARGO_BUILD_JOBS=2 cargo check -p aleph-panel --lib` — passed.
- `CARGO_BUILD_JOBS=2 cargo test -p aleph-panel --lib model_picker` — 6 passed.

The first attempt at the slash command test was stopped after 300 seconds during the Windows cold compile. The subsequent target-worktree run completed successfully after 7m55s. An earlier agent report that ran these commands from `D:/Workspace/Aleph` is excluded from this acceptance record.

## Review findings and resolutions

- Fixed the Task 1 durable generation race and metadata patch bypass before Task 2.
- Made generation-aware sink/store defaults fail closed and malformed generation handling reject rather than coerce to zero.
- Added refusal at selectable-model resolution and preserved the previous selection on rejection.
- Fixed the final integration review finding where declared plugin/command model ids could bypass the resolver as `ModelOverride::Raw`. Runner now resolves explicit per-turn/workflow targets through the same roster before constructing the override, and defers an unavailable flow fallback when a valid live session selection can still win.
- Durable selection control now awaits the generation-conditional store write and rolls the live map back when the sink rejects/fails; request provenance is recorded only after the delegated provider returns successfully.
- Cleaned stale documentation that described next-message/next-run activation and rollback wording inconsistent with request-boundary activation.

## Remaining risk

- The focused acceptance matrix is green. A complete workspace test was not repeated; the core test check, core clippy, Panel check, and all feature-focused tests passed.
- Existing repository test-only unused/dead-code warnings remain. No warning was treated as a feature failure.
- The Panel session action receives the existing `ChatSendResponse`, which has no generation field, so the UI reports the action as unconfirmed until the normal session snapshot/event feed reflects the accepted selection.

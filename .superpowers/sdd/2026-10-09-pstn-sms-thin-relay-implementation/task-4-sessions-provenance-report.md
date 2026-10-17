# Task 4 — sessions.send provenance slice (A)

## Problem
`src/builtin_tools/sessions/send_tool.rs` hard-coded `telecom_public: None` on the initial delegated `RunRequest` and on the `-continue` auto-continuation request. A public telecom parent run therefore delegated into an "ordinary" child (origin laundered); fire-and-forget additionally runs in a bare `tokio::spawn` (task-locals lost).

## Change
- `call_impl` captures `crate::gateway::telecom::run_origin::current_run_origin()` once, before any spawn, alongside the existing scope/room-author/turn-context captures. Source is the run-origin task-local, seeded only from the parent request's own admitted `telecom_public` (never a model argument).
- Both RunRequests (initial; auto-continuation) set `telecom_public: inherited_telecom_public.clone()` — an explicit typed field, so the fire-and-forget spawn carries it by value, not by task-local.
- Ordinary / unseeded parent reads `None` -> children stay `None`. No source_channel/metadata/permission/approval/turn-context logic touched; no upgrade of origin is possible (only copy of the parent's).
- `run_origin.rs` module doc updated (reader list only).

## Tests (real, observation-based; `send_tool.rs` tests)
`ProvenanceAdapter` is a real `ExecutionAdapter` recording `(run_id, request.telecom_public)` (the typed field) and writing a real assistant reply into the agent's FileSessionStore (first run replies with `[output truncated` so the real auto-continuation path fires). Driven through the real `SessionsSendTool` + `GatewayContext` + `AgentRegistry`.
- `a_public_parents_wait_send_and_auto_continuation_carry_its_origin`: initial and `-continue` both `Some(ctx)`.
- `a_public_parents_fire_and_forget_send_carries_its_origin_across_the_spawn`: bare-spawned child `Some(ctx)`.
- `an_ordinary_or_unseeded_parent_send_stays_none`: seeded-ordinary and unseeded, both modes (incl. continuation) -> `None`.
Red first: the two public tests failed with `left: None` before the fix.

## Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo test -p alephcore --lib sessions::send_tool` : 25 passed.
- `cargo test -p alephcore --lib telecom::` : 34 passed (run_origin censuses intact).
- `cargo clippy -p alephcore --lib --tests`: no diagnostics in touched files.
- `rustfmt --edition 2021` on touched files; `git diff --check` clean.

## Not done
- Other `telecom_public: None` entries (cron, heartbeat, team scheduler/dispatcher, announce, GUI, OpenAI, orchestrator) untouched by design.
- Other spawn-and-lose-task-local tools (e.g. `team_delegate`, other delegation tools) not audited here.
- No census pinning that every `RunRequest` built under a public run carries origin; no live/provider/real-comms run; full lib suite not run.
- `session_send` with a parent that has no run scope but a public origin elsewhere is not possible by construction (unseeded -> None).

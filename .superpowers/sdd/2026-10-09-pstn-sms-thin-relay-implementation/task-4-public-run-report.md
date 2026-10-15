# Task 4 — public SMS run isolation slice (report)

## Delivered
- `src/orchestrator/dispatch.rs`: `PublicRunRequest {ctx, text, run_id, prompt_version}` (no owner key/metadata/workspace), `PUBLIC_PROMPT_VERSION = "telecom-public-v1"`, default-deny `HarnessRunner::run_public_text` (`FlowError::InvalidConfig(PUBLIC_RUN_UNSUPPORTED)`).
- `src/orchestrator/harness_bridge/public_run.rs`: `AgentHarnessRunner::run_public_text_impl` uses only `default_provider.current()`; fixed `PUBLIC_SYSTEM_PROMPT` + current text; `NullToolService`; ephemeral in-memory `SqliteEventStore` + `InProcessActorSessionService`; no compactor/trace/result store/verifier/guardrails/epoch registrar; `NoopToolSignalSink`; max 2 iterations; reply buffered (events sender unused); cancel -> `FlowError::Cancelled`; empty -> `Internal`. Wired in `runner_impl.rs`.
- `src/gateway/telecom/public_admission.rs`: `PublicRunRegistry` (1 run per `binding_ref`, global cap 4, explicit principal `@telecom-public` with in-process token cap 200k), fixed notices, `cancel_binding` (stop handle), permit drop releases slot.
- `src/gateway/telecom/public_run.rs`: `run_public_sms` (admit -> run -> exactly one reply; nothing on cancel/failure; no-harness refuses, never owner fallback) + `PublicReplySink`.
- `ExecutionAdapter::public_harness()` (default `None`; `ExecutionEngine` returns orchestrator harness).
- Router: `inbound_router/mod.rs` branches on typed `telecom_public_from_message_meta` right after permission grant — before hooks, activity stamp, HITL, voice, slash, `/btw`, busy lane, durable journal. `executor.rs::handle_public_telecom`: `/stop` -> `cancel_binding`; all else plain text; reply via `channel_registry.send` on the inbound reply route.
- Census guard fix: `run_loop/tests.rs::constructs_run_request` now matches the whole identifier (`PublicRunRequest {` was a false positive) + pin test.

## Tests (actually run, `--test-threads=1`)
- `public_run` 11 passed; `public_admission` 5 passed; `run_public_text_is_default_deny` 1 passed; `the_producer_scan` 2 passed; `inbound_router` 78 passed; `harness_bridge` 131 passed; `orchestrator::` 292 passed; `gateway::telecom` 44 passed.
- `run_loop`, `visibility`, `execution_engine`, `census`: only the known pre-existing reds (project_skill_block_none_when_no_project_skills, btw no_shipped_command_word..., every_ambient_owner_call_site_is_in_the_census).
- TDD note: tests were written alongside the implementation, not run RED first.

## NOT done / not wired
- Public spend is NOT charged to the global `SpendLedger`/`MeteringProvider`; the metering floor may still attribute public provider calls to ambient/Unattributed. Cap is in-process, non-persistent, never resets.
- Caps are constants (no config surface).
- No router-level end-to-end test (branch tested via unit seams only); `build_context_with_agent`/agent resolution/link-access still run before the branch.
- Public `/stop` only cancels public runs; owner `/stop` does not reach public runs.
- No PSTN/voice public path.

## Risks
- Reply delivery failure is only logged.
- `PUBLIC_SYSTEM_PROMPT` wording unreviewed by product.

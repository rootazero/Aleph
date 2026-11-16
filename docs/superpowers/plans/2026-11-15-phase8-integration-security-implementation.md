# Phase8 Integration & Security Implementation Plan / Phase8 集成与安全实施计划

**Status / 状态:** approved for implementation by Task5 human gate (`m01055`)
**Date / 日期:** 2026-11-15
**Input / 输入:** [Phase8 design](../specs/2026-11-15-phase8-integration-security-design.md), [security evidence matrix](../runs/2026-11-15-phase8-security-evidence-matrix.md), [entry inventory](../runs/2026-11-15-phase8-entry-inventory.md)

## Goal and boundary / 目标与边界

Implement one bounded Capability Phase8 Integration & Security slice that proves fail-closed behavior across real Gateway, Core, renderer and PTY boundaries. Preserve `Zahir`/`ZahirFacade`, `OwnershipTree`, `PolicyEngine`, `InMemoryEventStream`, `ProjectionHost`, `UiSurface`, provider/source capture and PTY ownership authorities. Do not add a second registry, owner map, readiness latch, session store, universal scheduler, durable replay source, or new capability kind.

This track is independent from NSH. It does not implement native UI, change Tauri/WebChat, promote Contract A, or repair the recorded simulated transcript P2 without a separately approved durability contract.

## Concrete files and receipts / 具体文件与收据

The first implementation change set is limited to these proposed files:

| Purpose | File | Named symbols / receipt |
|---|---|---|
| Integrated Gateway/Core security cases | `tests/phase8_integration_security.rs` | `foreign_scope_denial_records_policy_violation_without_handler_effect`, `stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`, `ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`, `projection_lag_requires_snapshot_before_reconnect_resolve`, `disable_closes_owned_surface_and_rejects_old_lease` |
| Existing crate-internal PTY capture seam | `src/capability/zahir_facade.rs` | `captured_terminal_rejects_provider_replacement_before_invoke`; capture `OwnedPtySession` before the barrier, replace provider/source, then call `ZahirFacade::invoke_terminal_gateway(..., captured: Some(captured))` |
| Existing Gateway ingress proof | `src/gateway/capability_events_tests.rs` | retain a public `pty.*` ingress case; it must test the handler’s current capture path, not claim to pass an old capture through `handle_input_with_source` |
| Existing unit proof extended only if a gap is found | `src/capability/events_tests.rs` | reuse `policy_denial_records_requesting_actor_and_resource`, `lease_validation_denial_is_audited`, `lag_is_reported_instead_of_silently_skipped`; add only a terminal assertion if the integration fixture cannot observe it |
| Existing policy fixture extended only if a gap is found | `src/capability/policy_runtime_tests.rs` | reuse `trusted_run_actor_does_not_become_the_runtime_tool_owner`, `captured_owner_does_not_follow_an_unrelated_tree_rebind`, `source_callback_runs_outside_locks_and_binding_is_rechecked_after_it` |
| Existing projection lifecycle extended only if a gap is found | `src/capability/projection_host.rs` | reuse `stale_cursor_is_not_durable_recovery`, `revoked_lease_fails_closed_on_late_completion`, `cancelled_first_waiter_does_not_detach_teardown` |
| Existing Gateway event/PTY/UI boundary | `src/gateway/capability_events_tests.rs` | reuse `contextual_resolve_and_prepare_refusals_are_observed_at_the_actual_boundary`, `cancelled_ui_caller_keeps_owned_completion_and_close_releases_drain`, `token_cancel_does_not_drop_started_handler_or_release_stop_drain` |
| Evidence receipt | `docs/superpowers/runs/2026-11-15-phase8-security-execution-receipt.md` | future execution receipt: one row per named test with command, commit, terminal result, effect/resource state and limitations; this is distinct from the design matrix at `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md` |

The test file is a proposed new test artifact, not a new public API. Any helper in it must call existing authorities and retain their exact result types.

## Ordered implementation slices / 有序实施切片

### Slice A — F1 foreign owner/scope and F2 stale captures

**Ingress order:** authenticated Gateway request → typed parameter parsing → existing `CapabilityCallContext` → `Zahir::resolve`/`PolicyEngine::resolve` → capture `BackendLease` or existing UI/PTY attachment → controlled barrier → owner/provider/connection mutation through the existing authority → existing handler/`UiSurface`/PTY use → terminal response and event inspection.

**Mutation points:** `OwnershipTree::bump`, `rotate_capability`, `revoke` or `dispose`; existing provider/source replacement fixture; `UiSurface::close` or remount attachment invalidation. Do not mutate a shadow map.

**Cases and terminal assertions:**

- `foreign_scope_denial_records_policy_violation_without_handler_effect`: foreign principal/owner/workspace/kind/id reaches the real Gateway boundary; assert the domain denial, no handler/external effect and no successful output. Assert the expected scoped `PolicyViolation`/`ConstraintViolation` is present and cannot authorize a retry.
- `stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`: capture a lease, cross a barrier, call existing `OwnershipTree::bump`, use the old lease, then resolve fresh. Assert old use returns the existing stale/generation error, no old effect succeeds, and the fresh binding resolves.
- `captured_terminal_rejects_provider_replacement_before_invoke`: keep the public Gateway ingress assertion separate. The capture race belongs in the existing crate-internal test module in `src/capability/zahir_facade.rs`, where the test can use the existing test-only `require_owned_session_with_source` seam. Capture `OwnedPtySession` before a controlled barrier, replace the provider instance/generation or terminal source, then call the actual capture-receiving `ZahirFacade::invoke_terminal_gateway(..., captured: Some(captured))`. Assert `provider_matches`/source revalidation rejects the old operation before handler effect. Do not call crate-private `require_owned_session` from `tests/phase8_integration_security.rs`, and do not widen the production Gateway API. Reuse the existing fixture and semantic established by `src/gateway/capability_terminal_tests.rs:1157` (`gateway_facade_rejects_captured_session_after_provider_restart`); the new named case is only a source-replacement/barrier seam where that existing test does not already expose the required receipt, not a second authority.

- The public Gateway test separately exercises the current `handle_*_with_source` ingress, which re-captures at entry; it proves the live boundary and is not used to claim an old capture race.

### Slice B — F3 queued cancellation

**Ingress order:** `UiSurface::display` or existing special dispatch → pending request/worker creation → controlled barrier → cancellation or `UiSurface::close` → matching `UiRenderAck` attempt or worker completion → join and pending-state inspection.

**Cases:**

- `ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`: cancel after dispatch and before ACK; assert `UiRenderAck` cannot turn the request into success, `pending` is cleared, the worker join completes, and the result is cancelled/closed or the actual external-effect result if the effect crossed its start boundary.
- The same test fixture must cover pre-send cancellation and close with queued input as named subcases; record each subcase separately in the evidence receipt.

### Slice C — F4 withdrawal, lag and reconnect

**Ingress order:** `ZahirFacade::subscribe`/`ProjectionHost` subscribe-before-snapshot → publish bounded source changes → force the existing live stream to report `Lagged` or invalidation → obtain a complete fresh snapshot → reconnect/new consumer resolve. The current contract deliberately ignores the caller cursor and anchors at the current snapshot; preserve that contract.

**Case:** `projection_lag_requires_snapshot_before_reconnect_resolve`: assert `Lagged`/invalidation, a complete replacement snapshot revision and current-state resolution for the new consumer. Do not require an old cursor to be rejected, and do not claim that lag alone invalidates an unchanged lease. Test old lease refusal in the same or a separate case only after an explicit owner/provider/source change, using the existing `provider_restart_invalidates_old_tool_leases_and_hides_projection` or `owner_bump_rejects_old_lease_without_provider_restart` authority. The event record is not treated as replay or authorization.

### Slice D — F5 disable and cleanup

**Case:** `disable_closes_owned_surface_and_rejects_old_lease`: disable/close through `UiSurface::close` and the existing lifecycle owner while a display/input or projection operation is pending. Assert pending settlement, worker join, policy fact withdrawal, old lease refusal, and first cleanup error preservation. Repeated close must be terminal and harmless.

## Audit and effect semantics / 审计与效果语义

A refusal is expected to emit the existing scoped observation. `src/capability/policy_runtime.rs::observe_refusal` and `src/capability/events.rs` may produce `PolicyViolation` or `ConstraintViolation`; the tests must assert that those records carry the requesting actor/resource and cannot authorize work. The negative assertion is **no handler/external effect and no successful output/event**, never “no audit record.” `lease_id` remains correlation only.

Keep Contract A exactly as accepted: real addressed-key `chat.send`/`agent.run` commits canonical metadata before a successful response. Keep the reachable simulated `SimpleExecutionEngine` transcript P2 at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569`, `:1572`, `:1618` and `src/gateway/execution_engine/simple.rs`; no test here may upgrade simulated transcript durability.

## Validation protocol / 验证纪律

For every Cargo command, run immediately beforehand `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`; only exit 0 permits the command. Use a fresh isolated `CARGO_TARGET_DIR`, `--offline --locked --jobs 1`, bounded owned process groups and natural terminal receipts. Use real loopback Gateway/Core/renderer/PTY boundaries and deterministic controlled providers. Mark ignored, optional and not-run cases separately. Run focused Rust proof from the repository root with `CARGO_TARGET_DIR` set to a fresh task-specific directory: `cargo test -p alephcore --lib --offline --locked --jobs 1` and `cargo test -p alephcore --test phase8_integration_security --offline --locked --jobs 1`. The second command is required for the proposed `tests/phase8_integration_security.rs` target; the lib command alone does not execute it. Run the server binary compilation gate from the same cwd with `cargo check --bin aleph-server --offline --locked --jobs 1` when the implementation touches server startup. For Panel source tests, run the existing scoped scripts from `interfaces/webchat`: `bun run panel:tabs:test`, `bun run panel:tabs:artifacts:test`, and `bun run panel:tabs:protocol:test` as applicable; there is no `check` script in the root or `interfaces/webchat/package.json`. Before every Cargo command run `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`; only exit 0 permits the command. Use a fresh isolated target, offline/locked/jobs1, and recompute coverage using the corrected retained-LCOV method. Write all terminal outputs and effect/resource receipts to `docs/superpowers/runs/2026-11-15-phase8-security-execution-receipt.md`.

The proposed integration receipt must be JSON Lines or JSON with one object per case at `docs/superpowers/runs/2026-11-15-phase8-security-execution-receipt.md`. Each object has: `case`, `command`, `cwd_relative`, `commit`, `memory_gate_bytes`, `target_dir`, `terminal_status`, `ingress`, `barrier`, `mutation`, `attempted_effect`, `terminal_response`, `owned_state`, `audit_observation`, `limitations`. `terminal_status` is one of `PASS`, `FAIL`, `NOT_RUN`, `IGNORED`; a PASS requires the named effect/cleanup assertion, not merely that a call returned. The receipt path is future execution output and is deliberately distinct from the design evidence matrix.

Each slice is independently revertible. Reject implementation if it introduces a second authority, proves invocation rather than effect/cleanup, treats live notification as replay, collapses PTY and renderer acknowledgements, changes Contract A implicitly, or leaves owned work detached after disable. Native NSH, all-backend, remote, exactly-once, Safe Replay and universal scheduler claims remain outside this plan.

Approval applies only to the listed test/evidence slices and their necessary local test fixtures. It does not approve public wire changes, NSH dependency migration, Tauri/WebChat removal, release or push.

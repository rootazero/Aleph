# Phase8 Security Evidence Matrix / Phase8 安全证据矩阵

**Status / 状态:** design evidence map; no implementation claim
**Date / 日期:** 2026-11-15
**Design:** [Phase8 Integration & Security Design](../specs/2026-11-15-phase8-integration-security-design.md)
**Implementation plan:** [concrete Phase8 plan](../plans/2026-11-15-phase8-integration-security-implementation.md)

| ID | Proposed test symbol | Real ingress order | Mutation/barrier | Terminal assertion and expected observation | Evidence receipt |
|---|---|---|---|---|---|
| F1 foreign owner/scope | `tests/phase8_integration_security.rs::foreign_scope_denial_records_policy_violation_without_handler_effect` | authenticated Gateway request → typed params → `CapabilityCallContext` → `Zahir::resolve`/`PolicyEngine::resolve` → handler/UI/PTY boundary | foreign principal/owner/workspace/kind/id or connection before effect; use existing policy boundary | domain denial; no handler/external effect and no successful output; expected scoped `PolicyViolation`/`ConstraintViolation` exists and cannot authorize retry | command, commit, terminal result, effect counter/resource receipt, event record id and limitation in future `docs/superpowers/runs/2026-11-15-phase8-security-execution-receipt.md` |
| F2 stale lease/provider/connection | `tests/phase8_integration_security.rs::stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`; crate-internal `src/capability/zahir_facade.rs::captured_terminal_rejects_provider_replacement_before_invoke` | public Gateway ingress remains a separate current-capture test; crate-internal case captures `OwnedPtySession` before the barrier and calls `ZahirFacade::invoke_terminal_gateway(..., captured: Some(captured))` after provider/source replacement | `OwnershipTree::bump`/`rotate_capability`/revoke/dispose or existing provider/source replacement fixture | old lease/PTY source fails closed with existing stale/provider error; no old effect/output; fresh binding resolves and cleanup reaches terminal state | same receipt fields plus owner/provider generation values; no external test calls crate-private `require_owned_session`; reuse relationship to `src/gateway/capability_terminal_tests.rs:1157` (`gateway_facade_rejects_captured_session_after_provider_restart`) is recorded in the execution receipt |
| F3 queued cancellation | `tests/phase8_integration_security.rs::ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending` | `UiSurface::display`/special dispatch → pending worker → ACK/cancel/close | pre-send, post-send-before-ACK, admission and close/queued-input subcases cross a controlled barrier | pending request cleared; matching `UiRenderAck` cannot revive cancelled request; worker joins; result is cancel/close or actual started-effect result | join receipt, pending/queue state, ACK result, no successful post-cancel event |
| F4 withdrawal/lag/reconnect | `tests/phase8_integration_security.rs::projection_lag_requires_snapshot_before_reconnect_resolve`; existing `src/capability/zahir_facade.rs::subscribe_lagged_resyncs_to_fresh_snapshot` | `ZahirFacade::subscribe`/`ProjectionHost` subscribe-before-snapshot → live notification → reconnect/new resolve | bounded source changes force existing `Lagged`/invalidation; fresh snapshot follows; explicit owner/provider/source replacement is a separate mutation when testing lease refusal | `Lagged`/invalidation is observed, a complete replacement snapshot revision is obtained, and current resolution succeeds; caller cursor is ignored by the current contract; an unchanged lease is not rejected merely due to lag | snapshot revision, invalidation/lag error, current resolve result, and separate old/new lease evidence after real generation/source change |
| F5 disable/cleanup | `tests/phase8_integration_security.rs::disable_closes_owned_surface_and_rejects_old_lease` | existing lifecycle owner → `UiSurface::close`/source withdraw → pending operation settlement | close during display/input/projection operation; repeat close | pending/worker/resource terminal state; policy fact withdrawn; old handle refused; first cleanup error preserved | join/resource/fact receipts and original error |

## Existing proof to reuse / 可复用已有证明

- `src/capability/events_tests.rs`: `policy_denial_records_requesting_actor_and_resource`, `lease_validation_denial_is_audited`, `lag_is_reported_instead_of_silently_skipped`.
- `src/capability/policy_runtime_tests.rs`: `trusted_run_actor_does_not_become_the_runtime_tool_owner`, `captured_owner_does_not_follow_an_unrelated_tree_rebind`, `source_callback_runs_outside_locks_and_binding_is_rechecked_after_it`.
- `src/capability/projection_host.rs`: `stale_cursor_is_not_durable_recovery`, `revoked_lease_fails_closed_on_late_completion`, `cancelled_first_waiter_does_not_detach_teardown`.
- `src/gateway/capability_events_tests.rs`: `contextual_resolve_and_prepare_refusals_are_observed_at_the_actual_boundary`, `cancelled_ui_caller_keeps_owned_completion_and_close_releases_drain`, `token_cancel_does_not_drop_started_handler_or_release_stop_drain`.

These existing tests are supporting proof, not a claim that the new integrated terminal receipts already exist.

## Audit semantics / 审计语义

Refusals may and should emit scoped `PolicyViolation`/`ConstraintViolation` observations through `src/capability/policy_runtime.rs::observe_refusal` and `src/capability/events.rs`. The matrix requires absence of unauthorized handler/external effect or successful output, not absence of all audit records. Audit records never authorize work; `lease_id` is correlation only.

## Evidence construction rules / 证据构造规则

Use real loopback Gateway/Core/renderer/PTY boundaries, deterministic providers and bounded barriers. Record capture, mutation, attempted effect, terminal response, owned-task/resource state, audit result and limitations. Before every Cargo command run the 4GiB memory check; use fresh isolated `CARGO_TARGET_DIR`, `--offline --locked --jobs 1`, bounded processes and natural receipts. Preserve Contract A, the simulated transcript P2, live-only events and the renderer-ACK/PTY-reply distinction.

This matrix does not prove all backends, remote deployment, external-effect exactly-once, Safe Replay, universal scheduler, native NSH or Tauri removal.

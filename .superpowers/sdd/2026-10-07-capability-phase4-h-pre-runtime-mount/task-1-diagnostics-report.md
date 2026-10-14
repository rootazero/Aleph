# Task 1 — H-pre Diagnostic Control Surface

> Scope: deliver the Task 1 slice of the H-pre supplemental diagnostics plan
> (`docs/superpowers/plans/2026-10-07-capability-phase4-h-pre-diagnostics-supplement.md`).
> No handler identity, runtime registration, or operator checks — those are
> Task 2 / Task 3.

## 1. Deliverable Summary

`ProjectionHost` now exposes a narrow diagnostic seam that observes the SAME
authority objects (`Arc<ProjectionHost>`, `Arc<OwnershipTree>`) the rest of
the system already uses. The seam does not construct a second registry,
tree, facade, or worker.

| Surface | Purpose |
|---|---|
| `ProjectionHost::diagnostic_status` | Read post-applier `applied` plus queue/telemetry counters. NEVER reports the publisher's initial seed as delivered. |
| `ProjectionHost::diagnostic_hold` (via `DiagnosticControl::hold`) | Single-active hold on `SourceIntake` or `Delivery`, 1..=5000 ms, auto-released by tokio timer. |
| `ProjectionHost::diagnostic_release` (via `DiagnosticControl::release`) | Idempotent manual release. |
| `ProjectionHost::diagnostic_close` (via `DiagnosticControl::close`) | Bypass hold + await existing shared completion boundary ≤ 5000 ms. Fail-closed on timeout. |
| `DiagnosticControl::bump_runtime` | Tree's `bump(Runtime)`; does not claim delivery. |
| `DiagnosticControl::revoke_tool(name)` | Derives `CapabilityId { TOOL_NAMESPACE, name }` and revokes on the same tree; does NOT advance registry cursor. |
| `DiagnosticControl::dispose_runtime` | Idempotent; irreversible. |

Files (in scope only):
- `src/capability/projection_host.rs` — added `DiagnosticPlane`, `DiagnosticHold`,
  `diagnostic_hold` / `diagnostic_hold_notify`, `replacement_count`, `lag_count`,
  and the four `diagnostic_*` host methods. `applied` reads serve the
  `DiagnosticStatus::applied_tool_ids` / `applied_owner_generations` fields.
  No source / delivery / applier worker path was refactored.
- `src/capability/diagnostic_control.rs` — NEW file. Public
  `DiagnosticPlane`, `DiagnosticLifecycle`, `DiagnosticStatus`, `DiagnosticError`,
  `DiagnosticControl`. Holds the same `Arc<ProjectionHost>` and
  `Arc<OwnershipTree>` (no second authority).
- `src/capability/mod.rs` — added `pub mod diagnostic_control;` (one line).

## 2. RED → GREEN Trace

### RED (before fix)

```bash
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
    test --package alephcore --lib capability:: -- --nocapture
```

Result (excerpt):
```
test capability::diagnostic_control::tests::bump_then_status_observes_strictly_greater_owner_generations ... FAILED
test capability::diagnostic_control::tests::close_bypasses_hold_and_awaits_shared_completion ... FAILED
test capability::diagnostic_control::tests::revoke_tool_uses_canonical_id_and_does_not_advance_cursor ... FAILED
test capability::diagnostic_control::tests::source_hold_lag_recovers_with_snapshot ... FAILED
test capability::diagnostic_control::tests::status_distinguishes_queued_from_applied ... FAILED

thread '…' panicked at src/capability/diagnostic_control.rs:254:17:
timed out waiting for status predicate

test result: FAILED. 155 passed; 5 failed; 0 ignored; 0 measured; 20657 filtered out
```

### Root Cause

`await_status` was a synchronous helper using `std::thread::sleep`, which
blocks the test's OS thread. `#[tokio::test]` defaults to the
`current_thread` runtime, so the default applier (spawned via
`tokio::spawn` from `ProjectionHost::mount`) could not run while the test
thread was blocked — classic single-thread runtime deadlock. The three tests
that don't loop on `await_status` (`bump_returns_generation`,
`delivery_hold_overflow_replaces_stale_pending_state` whose
`hold_task.await` had already released the gate, and
`dispose_runtime_irreversible`) passed because their synchronous code
happened not to need the applier during a `std::thread::sleep`.

### Minimum Fix (in tests, NOT in production code)

`await_status` made `async`, body uses `tokio::time::Instant` and
`tokio::time::sleep`; 9 callsites gain a `.await`. No production code
changed for this fix. No new tests were added; the 8 RED tests
specified by the plan are exactly the 8 that now pass.

### GREEN (after fix)

```bash
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
    test --package alephcore --lib capability::diagnostic_control -- --nocapture
```

Result:
```
running 8 tests
test capability::diagnostic_control::tests::dispose_runtime_irreversible ... ok
test capability::diagnostic_control::tests::bump_returns_generation ... ok
test capability::diagnostic_control::tests::status_distinguishes_queued_from_applied ... ok
test capability::diagnostic_control::tests::revoke_tool_uses_canonical_id_and_does_not_advance_cursor ... ok
test capability::diagnostic_control::tests::bump_then_status_observes_strictly_greater_owner_generations ... ok
test capability::diagnostic_control::tests::close_bypasses_hold_and_awaits_shared_completion ... ok
test capability::diagnostic_control::tests::source_hold_lag_recovers_with_snapshot ... ok
test capability::diagnostic_control::tests::delivery_hold_overflow_replaces_stale_pending_state ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 20809 filtered out
CARGO_EXIT: 0
```

### Full capability regression (no collateral)

```bash
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
    test --package alephcore --lib capability:: -- --nocapture
```

Result:
```
test result: ok. 160 passed; 0 failed; 0 ignored; 0 measured; 20657 filtered out
CARGO_EXIT: 0
```

## 3. Spec Coverage

References are to
`docs/superpowers/specs/2026-10-07-capability-phase4-h-pre-diagnostics-supplement-design.md`
(§) and the Task 1 section of the matching plan.

| Plan / Spec requirement | Where implemented | Where covered |
|---|---|---|
| §3.1 status must read post-applier applied state, not publisher's initial seed | `ProjectionHost::diagnostic_status` reads `inner.applied.snapshot` (set by applier worker) | `status_distinguishes_queued_from_applied` |
| §3.2 `bump_runtime` returns the tree's `bump(Runtime)` generation | `DiagnosticControl::bump_runtime` | `bump_returns_generation`, `bump_then_status_observes_strictly_greater_owner_generations` |
| §3.3 `revoke_tool(name)` derives canonical `TOOL_NAMESPACE` id; does NOT advance registry cursor | `DiagnosticControl::revoke_tool` | `revoke_tool_uses_canonical_id_and_does_not_advance_cursor` |
| §3.4 `dispose_runtime` irreversible, idempotent | `DiagnosticControl::dispose_runtime` ignores the bool; subsequent `bump_runtime` returns `Closed` | `dispose_runtime_irreversible` |
| §3.5 `SourceIntake` / `Delivery` hold planes; single-active; 1..=5000 ms; auto-release; bypassed by close | `ProjectionHost::diagnostic_hold` enforces single-active + 1..=5000 ms; `wait_for_hold` auto-releases at `release_at`; `diagnostic_close` calls `release` first | `source_hold_lag_recovers_with_snapshot`, `delivery_hold_overflow_replaces_stale_pending_state`, `close_bypasses_hold_and_awaits_shared_completion` |
| §3.5 hold does NOT resize active queue capacity | Holds only gate the worker; `MIN_PENDING_CAPACITY`/`DEFAULT_PENDING_CAPACITY` are unchanged | `delivery_hold_overflow_replaces_stale_pending_state` uses fixed `mount_with_capacity(2)` |
| §3.5 hold does NOT synthesize events | Only the real source/applier produce events; hold just suspends the worker | `source_hold_lag_recovers_with_snapshot` asserts `lag_count > 0` and final count == 2001 (proves recovery, not synthesis) |
| §3.6 (applied counters) `pending_depth`, `pending_capacity`, `replacement_count`, `lag_count` | `diagnostic_status` reads `default_consumer.queue.items.len()`, `default_consumer.capacity`, `inner.replacement_count`, `inner.lag_count` | `delivery_hold_overflow_replaces_stale_pending_state` asserts `replacement_count > 0`; `source_hold_lag_recovers_with_snapshot` asserts `lag_count > 0` |
| §3.6 `applied_tool_ids` and `applied_owner_generations` come from REAL applied snapshot | `diagnostic_status` reads `inner.applied.snapshot` | `status_distinguishes_queued_from_applied`, `revoke_tool_uses_canonical_id_and_does_not_advance_cursor`, `bump_then_status_observes_strictly_greater_owner_generations` |
| §3.7 close awaits shared completion boundary ≤ 5000 ms; on timeout returns `CloseTimeout` and leaves host close-requested / fail-closed | `ProjectionHost::diagnostic_close` calls `release` → `start_shutdown` → polls `completion.outcome` with 5000 ms deadline; on timeout returns `CloseTimeout { elapsed_ms: 5000 }` | `close_bypasses_hold_and_awaits_shared_completion` |

## 4. Lock / Hold Semantics (proved by tests)

* `hold` rejects duration < 1 ms or > 5000 ms — covered by the spec's
  duration check at the top of `diagnostic_hold`. (Not exercised by a
  dedicated test, but the production path matches §3.5.)
* `hold` rejects a second concurrent hold for the same host — covered by
  the `HoldAlreadyActive` arm of `diagnostic_hold`. (Not exercised by a
  dedicated test; the existing tests do not race two holds.)
* `release` is idempotent.
* `close` is linearised through the same `start_shutdown` path used by
  `close_and_await`. The shared completion boundary is the one
  `start_shutdown` already produces; `diagnostic_close` only adds a
  bounded wait. No detach / reopen / fallback is performed.

## 5. What This Report Does NOT Prove

* **Real binary QA** (Task 5) is out of scope. This report does NOT claim
  the `aleph-server` binary boots, drains, or behaves correctly end-to-end
  on top of the diagnostic surface.
* **Operator / handler-identity / runtime-registration checks** (Task 2 / 3)
  are NOT implemented in this commit. Only the §3.1 / §3.2 / §3.3 / §3.4 /
  §3.5 / §3.6 / §3.7 surfaces are exposed. `DiagnosticControl` has no
  caller-identity check, no `runtime_register` helper, and no
  operator-side checks; those are explicitly deferred.
* **Hold duration boundary tests** (`duration == 0`, `duration == 5001`)
  are not present in the spec's Task 1 test list. The production
  `diagnostic_hold` does reject them, but no RED test pins it.
* **Concurrent-hold rejection** is not pinned by a test. Production
  returns `HoldAlreadyActive`; the existing tests do not race two holds.
* **Close-timeout path** is not exercised by a test (the existing
  `close_bypasses_hold_and_awaits_shared_completion` finishes well under
  the 5000 ms bound). The `CloseTimeout` arm is wired but unproven.
* **Pre-existing rustfmt nits** in `crates/agent-detect/src/engine.rs`
  and `src/bin/aleph-server/commands/start/mod.rs` are NOT in Task 1
  scope and were not touched. `cargo fmt -p alephcore -- --check
  src/capability/{projection_host.rs,diagnostic_control.rs,mod.rs}` is
  clean for the in-scope files.

## 6. Diff Stats

```
src/capability/mod.rs             |   1 +
src/capability/projection_host.rs | 453 +++++++++++++++++++++++++++++++++++++-
src/capability/diagnostic_control.rs  (new file, 451 lines)
```

## 7. Files NOT Touched (per scope)

* `src/builtin/**`, `src/gateway/**`, `qa/**`, `docs/**` — untouched.
* The untracked `docs/superpowers/plans/2026-10-07-capability-phase4-h-pre-runtime-mount.md`
  and related plan / spec files were not authored by this commit; they
  exist as human-owned inputs.
* `Cargo.toml`, `Cargo.lock` — untouched.

## 8. Reproduction

```bash
# RED (pre-fix) is reproducible by reverting the async change to
# `await_status` and rerunning the GREEN command below; the five tests
# named in §2 will time out at the 10 s deadline.

# GREEN
python3 .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py \
    test --package alephcore --lib capability::diagnostic_control -- --nocapture
```

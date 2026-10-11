# Task 4 Public Approval Edge Report

## Scope

- Touched source/tests: `src/gateway/telecom/public_approval.rs` only.
- Report: `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-public-approval-edge-report.md`.
- Excluded paths intentionally not edited/staged: `src/gateway/telecom/public_run.rs`, `src/gateway/telecom/public_history.rs`, spend/router files, and pre-existing dirty docs.

## Fixes

1. `PublicApprovalLedger::request_disclosure` no longer marks a consumed read approval as `disclosure_claimed` before insertion capacity is known.
   - It now prunes expired/retired records, validates the consumed read immutably, checks `MAX_APPROVAL_RECORDS`, inserts B, then marks the read claimed.
   - A full-ledger `PublicApprovalError::Capacity` refusal leaves A consumable for a later retry, preserving one-shot semantics.
2. `PrivateDisclosureSink::deliver` now returns `Result<(), PublicApprovalError>` and `deliver_private_disclosure` propagates the sink result.
   - Added `PublicApprovalError::DeliveryFailed`.
   - The fence still consumes B before touching the sink, and a sink failure is not reported as successful delivery.

## Regression coverage

- `disclosure_capacity_refusal_does_not_claim_consumed_read`: fills the ledger with unrelated records, verifies B request returns `Capacity`, frees unrelated capacity, then reuses the same consumed read to deliver `PRIVATE-CANARY-vault-code-7731`.
- `sink_failure_is_reported_as_failed_delivery`: a failing sink sees `PRIVATE-CANARY-vault-code-7731`, returns `DeliveryFailed`, and the caller receives an error rather than success; replay is refused as `AlreadyConsumed`.

## Validation

- `rustfmt --edition 2021 src/gateway/telecom/public_approval.rs`: passed.
- `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2 timeout -k 20 900 setsid cargo check`: passed in 6m07s with pre-existing warnings:
  - `unused import: AgentDef` in `src/gateway/handlers/tools_invoke.rs:39:21`
  - `FastPathJournal::new` dead code in `src/gateway/execution_engine/fast_path.rs:37:19`
  - `ChannelRegistry::outbound_delivery_policy` dead code in `src/gateway/channel_registry.rs:410:25`
- `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2 timeout -k 20 900 setsid cargo test public_approval --lib`: attempted twice; both timed out with exit code 124 while compiling/linking the `alephcore` lib test harness, before any public_approval test result was emitted. Orphaned `rustc` children from the `setsid`/`timeout` combination were killed after each timeout.

## Residual integration gaps

- Public approval is still policy/capability core only: no authenticated operator UI wiring, no public run driver call path, and no production private disclosure channel sink are wired here.
- Sink failure propagation is now representable at this seam, but B is consumed before sink invocation; this avoids duplicate disclosure attempts after any sink acknowledgement path but cannot make an external side effect transactionally atomic.
- Full targeted test execution remains unverified because the requested 900s command timed out during test-harness compilation/linking on this worktree.
